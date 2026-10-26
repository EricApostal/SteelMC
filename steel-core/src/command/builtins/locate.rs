//! Structure, biome, and POI location commands.

use std::{sync::Arc, time::Instant};

use steel_registry::poi::PoiTypeRef;
use steel_utils::{BlockPos, Identifier, translations};
use text_components::{
    Modifier, TextComponent,
    format::Color,
    interactivity::{ClickEvent, HoverEvent},
    translation::Translation,
};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        BiomeOrTag, CommandResultSuspension, CommandResultSuspensionPoll, CommandSource,
        PoiTypeOrTag, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        StructureOrTagKey, argument, literal,
    },
    registration::CommandRegistration,
};
use crate::{
    chunk::{
        chunk_request::{ChunkRequest, ChunkRequestHandle, ChunkRequestState, ChunkTicketKind},
        status::ChunkStatus,
    },
    poi::poi_storage::{OccupationStatus, PointOfInterestStorage},
    world::World,
    worldgen::{
        generator::{ChunkGenerator, ClosestBiomeSearch},
        structure::{StructureLocateCandidate, StructureLocatePlan, squared_distance},
    },
};

const MAX_STRUCTURE_SEARCH_RADIUS: i32 = 100;

/// Vanilla `LocateCommand.MAX_BIOME_SEARCH_RADIUS`: the horizontal search radius
/// (in blocks) passed to `ServerLevel.findClosestBiome3d`.
const MAX_BIOME_SEARCH_RADIUS: i32 = 6400;

/// Vanilla `LocateCommand.BIOME_SAMPLE_RESOLUTION_HORIZONTAL`.
const BIOME_SAMPLE_RESOLUTION_HORIZONTAL: i32 = 32;

/// Vanilla `LocateCommand.BIOME_SAMPLE_RESOLUTION_VERTICAL`.
const BIOME_SAMPLE_RESOLUTION_VERTICAL: i32 = 64;

/// Vanilla `LocateCommand.POI_SEARCH_RADIUS`: the search radius (in blocks) passed
/// to `PoiManager.findClosestWithType` with `Occupancy.ANY`.
const POI_SEARCH_RADIUS: i32 = 256;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("locate"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("locate")
        .then(
            literal("structure").then(
                argument("structure", SteelArgumentType::structure_or_tag_key())
                    .executes_suspended(start_structure_search),
            ),
        )
        .then(literal("biome").then(
            argument("biome", SteelArgumentType::biome_or_tag()).executes(execute_biome_search),
        ))
        .then(
            literal("poi").then(
                argument("poi", SteelArgumentType::poi_or_tag()).executes(execute_poi_search),
            ),
        )
}

fn execute_biome_search(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let query = context.biome_or_tag("biome")?.clone();
    let world = Arc::clone(context.source().world());
    let origin = BlockPos::from(context.source().position());
    let started_at = Instant::now();

    let found = world
        .chunk_map
        .world_gen_context
        .generator
        .find_closest_biome_3d(
            ClosestBiomeSearch::new(
                origin,
                MAX_BIOME_SEARCH_RADIUS,
                BIOME_SAMPLE_RESOLUTION_HORIZONTAL,
                BIOME_SAMPLE_RESOLUTION_VERTICAL,
                world.get_min_y(),
                world.get_max_y(),
            )
            .allowing(&|biome| query.matches(biome)),
        );

    let Some((pos, biome)) = found else {
        return Err(biome_not_found(&query));
    };
    let name = query.found_name(&biome.key);
    let distance = three_dimensional_distance(origin, pos);
    Ok(show_locate_result(
        context.source(),
        &translations::COMMANDS_LOCATE_BIOME_SUCCESS,
        name,
        pos,
        distance,
        true,
        started_at,
    ))
}

fn execute_poi_search(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let query = context.poi_or_tag("poi")?.clone();
    let world = Arc::clone(context.source().world());
    let origin = BlockPos::from(context.source().position());
    let started_at = Instant::now();

    // Steel-only difference: vanilla's `SectionStorage.getOrLoad` loads POI sections
    // from disk on demand, while `poi_storage` only holds sections of loaded chunks; if
    // the matching POI sits in an unloaded chunk, the search reports not-found.
    let found = {
        let storage = world.poi_storage.lock();
        search_poi(&storage, &query, origin)
    };

    let Some((poi_type, pos)) = found else {
        return Err(poi_not_found(&query));
    };
    let name = query.found_name(&poi_type.key);
    let distance = horizontal_distance(origin, pos);
    Ok(show_locate_result(
        context.source(),
        &translations::COMMANDS_LOCATE_POI_SUCCESS,
        name,
        pos,
        distance,
        false,
        started_at,
    ))
}

/// Vanilla `LocateCommand.locatePoi`'s search (`ServerLevel.getPoiManager`'s
/// `findClosestWithType`): the closest POI matching `query` within
/// `POI_SEARCH_RADIUS` of `origin`, measured by 3D squared distance, with
/// `Occupancy.ANY`.
fn search_poi(
    storage: &PointOfInterestStorage,
    query: &PoiTypeOrTag,
    origin: BlockPos,
) -> Option<(PoiTypeRef, BlockPos)> {
    storage.find_closest_with_type(
        &|poi_type| query.matches(poi_type),
        origin,
        POI_SEARCH_RADIUS,
        OccupationStatus::Any,
    )
}

/// Vanilla `LocateCommand.showLocateResult`: sends the success message for a located
/// element and returns the located (floored) distance as the command result.
fn show_locate_result(
    source: &CommandSource,
    success: &Translation<3>,
    name: String,
    pos: BlockPos,
    distance: i32,
    show_y: bool,
    started_at: Instant,
) -> i32 {
    let displayed_y = if show_y {
        pos.0.y.to_string()
    } else {
        "~".to_owned()
    };
    let component = success
        .message([
            TextComponent::from(name.clone()),
            locate_coordinates_component(pos, displayed_y),
            TextComponent::from(distance.to_string()),
        ])
        .component();
    source.send_success(&component, false);
    tracing::info!(
        "Locating element {} took {} ms",
        name,
        started_at.elapsed().as_millis()
    );
    distance
}

fn start_structure_search(
    context: &SteelCommandContext<CommandSource>,
) -> Result<LocateStructureSearch, CommandSyntaxError> {
    let query = context.structure_or_tag_key("structure")?;
    let Some(structures) = query.resolve() else {
        return Err(invalid_structure(query));
    };
    if structures.is_empty() {
        return Err(structure_not_found(query));
    }

    let world = context.source().world();
    let Some(structure_generator) = world
        .chunk_map
        .world_gen_context
        .generator
        .structure_generator()
    else {
        return Err(structure_not_found(query));
    };
    let structure_keys = structures
        .iter()
        .map(|structure| structure.key.clone())
        .collect::<Vec<_>>();
    let Some(plan) = structure_generator.locate_plan_for_structures(&structure_keys) else {
        return Err(structure_not_found(query));
    };
    if plan.is_empty() {
        return Err(structure_not_found(query));
    }

    Ok(LocateStructureSearch {
        source: context.source().clone(),
        world: Arc::clone(world),
        query: query.clone(),
        plan,
        origin: BlockPos::from(context.source().position()),
        phase: LocatePhase::Start,
        pending: None,
        candidates: Vec::new(),
        best: None,
        random_radius: 0,
        started_at: Instant::now(),
    })
}

enum LocatePhase {
    Start,
    WaitingRings,
    RandomSpread,
    WaitingRandomSpread,
}

struct LocatedStructure {
    candidate: StructureLocateCandidate,
    found_structure: Identifier,
    distance_sqr: i64,
}

struct LocateStructureSearch {
    source: CommandSource,
    world: Arc<World>,
    query: StructureOrTagKey,
    plan: StructureLocatePlan,
    origin: BlockPos,
    phase: LocatePhase,
    pending: Option<ChunkRequestHandle>,
    candidates: Vec<StructureLocateCandidate>,
    best: Option<LocatedStructure>,
    random_radius: i32,
    started_at: Instant,
}

impl CommandResultSuspension for LocateStructureSearch {
    fn poll(&mut self) -> CommandResultSuspensionPoll {
        loop {
            match self.phase {
                LocatePhase::Start => {
                    self.candidates = self.plan.ring_candidates(self.origin);
                    if self.candidates.is_empty() {
                        self.phase = LocatePhase::RandomSpread;
                        continue;
                    }
                    self.pending = Some(self.request_current_candidates());
                    self.phase = LocatePhase::WaitingRings;
                    return CommandResultSuspensionPoll::Pending;
                }
                LocatePhase::WaitingRings => match self.poll_pending_request() {
                    PendingRequest::Pending => return CommandResultSuspensionPoll::Pending,
                    PendingRequest::Cancelled => return Self::cancelled_result(),
                    PendingRequest::Ready => {
                        self.best = self.first_valid_candidate();
                        self.clear_request();

                        if self.best.is_some() && !self.plan.has_random_spread() {
                            return self.success_result();
                        }

                        self.phase = LocatePhase::RandomSpread;
                    }
                },
                LocatePhase::RandomSpread => {
                    if self.random_radius > MAX_STRUCTURE_SEARCH_RADIUS {
                        return self.finished_result();
                    }

                    self.candidates = self
                        .plan
                        .random_spread_candidates_at_radius(self.origin, self.random_radius);
                    self.random_radius += 1;

                    if self.candidates.is_empty() {
                        continue;
                    }

                    self.pending = Some(self.request_current_candidates());
                    self.phase = LocatePhase::WaitingRandomSpread;
                    return CommandResultSuspensionPoll::Pending;
                }
                LocatePhase::WaitingRandomSpread => match self.poll_pending_request() {
                    PendingRequest::Pending => return CommandResultSuspensionPoll::Pending,
                    PendingRequest::Cancelled => return Self::cancelled_result(),
                    PendingRequest::Ready => {
                        if self.update_best_after_random_radius() {
                            return self.success_result();
                        }

                        self.clear_request();
                        self.phase = LocatePhase::RandomSpread;
                    }
                },
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.cancel();
        }
    }
}

impl LocateStructureSearch {
    fn request_current_candidates(&self) -> ChunkRequestHandle {
        let positions = self
            .candidates
            .iter()
            .map(|candidate| candidate.chunk_pos)
            .collect();
        self.world.chunk_map.request_chunks(ChunkRequest {
            status: ChunkStatus::StructureStarts,
            positions,
            ticket_kind: ChunkTicketKind::StructureLocate,
        })
    }

    fn poll_pending_request(&self) -> PendingRequest {
        let Some(pending) = &self.pending else {
            return PendingRequest::Cancelled;
        };

        match pending.poll() {
            ChunkRequestState::Pending { .. } => PendingRequest::Pending,
            ChunkRequestState::Ready => PendingRequest::Ready,
            ChunkRequestState::Cancelled => PendingRequest::Cancelled,
        }
    }

    fn clear_request(&mut self) {
        self.pending = None;
        self.candidates.clear();
    }

    fn first_valid_candidate(&self) -> Option<LocatedStructure> {
        self.candidates.iter().copied().find_map(|candidate| {
            self.generated_structure_at_candidate(candidate)
                .map(|found_structure| LocatedStructure {
                    candidate,
                    found_structure,
                    distance_sqr: squared_distance(candidate.locate_pos, self.origin),
                })
        })
    }

    fn update_best_after_random_radius(&mut self) -> bool {
        let mut best = self.best.take();
        let mut current_scan = None;
        let mut found_current_scan = false;
        let mut found_in_this_radius = false;

        for candidate in &self.candidates {
            if current_scan != Some(candidate.scan_id()) {
                current_scan = Some(candidate.scan_id());
                found_current_scan = false;
            }

            if found_current_scan {
                continue;
            }

            let Some(found_structure) = self.generated_structure_at_candidate(*candidate) else {
                continue;
            };
            found_current_scan = true;
            found_in_this_radius = true;
            let located = LocatedStructure {
                candidate: *candidate,
                found_structure,
                distance_sqr: squared_distance(candidate.locate_pos, self.origin),
            };
            if best
                .as_ref()
                .is_none_or(|current| located.distance_sqr < current.distance_sqr)
            {
                best = Some(located);
            }
        }

        self.best = best;
        found_in_this_radius
    }

    fn generated_structure_at_candidate(
        &self,
        candidate: StructureLocateCandidate,
    ) -> Option<Identifier> {
        let holder = self
            .world
            .chunk_map
            .chunks
            .read_sync(&candidate.chunk_pos, |_, holder| Arc::clone(holder))?;
        let chunk = holder.try_chunk(ChunkStatus::StructureStarts)?;
        let starts = chunk.structure_starts();
        let structures = self.plan.structures_for_candidate(candidate)?;
        structures.iter().find_map(|structure| {
            starts
                .get(structure)
                .is_some_and(|start| !start.pieces.is_empty())
                .then(|| structure.clone())
        })
    }

    fn finished_result(&self) -> CommandResultSuspensionPoll {
        if self.best.is_some() {
            self.success_result()
        } else {
            CommandResultSuspensionPoll::Ready(Err(structure_not_found(&self.query)))
        }
    }

    fn success_result(&self) -> CommandResultSuspensionPoll {
        let Some(best) = &self.best else {
            return CommandResultSuspensionPoll::Ready(Err(structure_not_found(&self.query)));
        };
        let pos = best.candidate.locate_pos;
        let distance = horizontal_distance(self.origin, pos);
        let name = self.query.found_name(&best.found_structure);
        CommandResultSuspensionPoll::Ready(Ok(show_locate_result(
            &self.source,
            &translations::COMMANDS_LOCATE_STRUCTURE_SUCCESS,
            name,
            pos,
            distance,
            false,
            self.started_at,
        )))
    }

    fn cancelled_result() -> CommandResultSuspensionPoll {
        CommandResultSuspensionPoll::Ready(Err(CommandSyntaxError::dynamic(
            "Structure search was cancelled",
        )))
    }
}

enum PendingRequest {
    Pending,
    Ready,
    Cancelled,
}

fn invalid_structure(query: &StructureOrTagKey) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_LOCATE_STRUCTURE_INVALID
            .message([TextComponent::from(query.as_printable())])
            .component(),
    )
}

fn structure_not_found(query: &StructureOrTagKey) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_LOCATE_STRUCTURE_NOT_FOUND
            .message([TextComponent::from(query.as_printable())])
            .component(),
    )
}

fn biome_not_found(query: &BiomeOrTag) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_LOCATE_BIOME_NOT_FOUND
            .message([TextComponent::from(query.as_printable())])
            .component(),
    )
}

fn poi_not_found(query: &PoiTypeOrTag) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_LOCATE_POI_NOT_FOUND
            .message([TextComponent::from(query.as_printable())])
            .component(),
    )
}

fn horizontal_distance(a: BlockPos, b: BlockPos) -> i32 {
    let dx = b.0.x.wrapping_sub(a.0.x);
    let dz = b.0.z.wrapping_sub(a.0.z);
    let squared = dx.wrapping_mul(dx).wrapping_add(dz.wrapping_mul(dz));
    (f64::from(squared as f32).sqrt() as f32).floor() as i32
}

/// Vanilla 3D locate distance: the floor of the float square root of the double
/// `Vec3i.distSqr`, which is what `locate biome` (the only `showY` branch)
/// reports and returns.
fn three_dimensional_distance(a: BlockPos, b: BlockPos) -> i32 {
    let dx = f64::from(b.0.x) - f64::from(a.0.x);
    let dy = f64::from(b.0.y) - f64::from(a.0.y);
    let dz = f64::from(b.0.z) - f64::from(a.0.z);
    let squared = dx * dx + dy * dy + dz * dz;
    (squared as f32).sqrt().floor() as i32
}

fn locate_coordinates_component(pos: BlockPos, displayed_y: String) -> TextComponent {
    TextComponent::plain("[")
        .add_child(
            translations::CHAT_COORDINATES
                .message([
                    TextComponent::from(pos.0.x.to_string()),
                    TextComponent::from(displayed_y.clone()),
                    TextComponent::from(pos.0.z.to_string()),
                ])
                .component(),
        )
        .add_child(TextComponent::plain("]"))
        .color(Color::Green)
        .hover_event(HoverEvent::show_text(
            &translations::CHAT_COORDINATES_TOOLTIP,
        ))
        .click_event(ClickEvent::suggest_command(format!(
            "/tp @s {} {} {}",
            pos.0.x, displayed_y, pos.0.z
        )))
}

#[cfg(test)]
mod tests {
    use super::super::create_dispatcher;
    use super::*;
    use crate::command::{
        brigadier::{CommandDispatcher, NodeId},
        execution::SteelCommandRuntime,
    };
    use steel_registry::init_vanilla_registry;

    type Dispatcher = CommandDispatcher<CommandSource, SteelCommandRuntime>;

    fn child(dispatcher: &Dispatcher, parent: NodeId, name: &str) -> NodeId {
        let Some(children) = dispatcher.children(parent) else {
            panic!("parent node should exist");
        };
        let Some(child) = children.iter().copied().find(|child| {
            dispatcher
                .node(*child)
                .is_some_and(|node| node.name() == name)
        }) else {
            panic!("child {name} should exist");
        };
        child
    }

    #[test]
    fn locate_graph_exposes_the_typed_structure_biome_and_poi_branches() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let locate = child(&dispatcher, dispatcher.root(), "locate");

        let structure = child(&dispatcher, locate, "structure");
        let structure_target = child(&dispatcher, structure, "structure");
        assert_eq!(
            dispatcher
                .node(structure_target)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::structure_or_tag_key())
        );
        assert_executable_leaf(&dispatcher, structure_target);

        let biome = child(&dispatcher, locate, "biome");
        let biome_target = child(&dispatcher, biome, "biome");
        assert_eq!(
            dispatcher
                .node(biome_target)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::biome_or_tag())
        );
        assert_executable_leaf(&dispatcher, biome_target);

        let poi = child(&dispatcher, locate, "poi");
        let poi_target = child(&dispatcher, poi, "poi");
        assert_eq!(
            dispatcher
                .node(poi_target)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::poi_or_tag())
        );
        assert_executable_leaf(&dispatcher, poi_target);

        assert_eq!(dispatcher.children(locate).map(<[_]>::len), Some(3));
    }

    fn assert_executable_leaf(dispatcher: &Dispatcher, node: NodeId) {
        let Some(argument_node) = dispatcher.node(node) else {
            panic!("locate argument should exist");
        };
        assert!(argument_node.is_executable());
        assert!(dispatcher.children(node).is_some_and(<[_]>::is_empty));
    }

    #[test]
    fn locate_coordinates_component_matches_vanilla_interactivity() {
        let component = locate_coordinates_component(BlockPos::new(12, 0, -34), "~".to_owned());

        assert_eq!(component.format.color, Some(Color::Green));
        assert!(matches!(
            component.interactions.click,
            Some(ClickEvent::SuggestCommand { ref command })
                if command.as_ref() == "/tp @s 12 ~ -34"
        ));
        assert!(matches!(
            component.interactions.hover,
            Some(HoverEvent::ShowText { .. })
        ));
    }

    #[test]
    fn horizontal_distance_matches_vanillas_wrapping_int_and_float_math() {
        assert_eq!(
            horizontal_distance(BlockPos::new(0, 0, 0), BlockPos::new(3, 100, 4)),
            5
        );
        assert_eq!(
            horizontal_distance(
                BlockPos::new(-30_000_000, 0, 0),
                BlockPos::new(30_000_000, 0, 0)
            ),
            36_907
        );
    }

    #[test]
    fn three_dimensional_distance_matches_vanillas_float_floor_math() {
        // `Mth.floor(Mth.sqrt((float) distSqr))`: (3, 4, 12) has exact distance 13.
        assert_eq!(
            three_dimensional_distance(BlockPos::new(1, 2, 3), BlockPos::new(4, 6, 15)),
            13
        );
        // (1, 1, 1) has distance sqrt(3); vanilla truncates to 1.
        assert_eq!(
            three_dimensional_distance(BlockPos::new(0, 0, 0), BlockPos::new(1, 1, 1)),
            1
        );
    }

    #[test]
    fn biome_checkerboard_search_finds_the_deterministic_spiral_offset() {
        use steel_registry::{biome::BiomeRef, vanilla_biomes};
        use steel_worldgen::biomes::BiomeSourceKind;

        init_vanilla_registry();
        let source =
            BiomeSourceKind::checkerboard(vec![&vanilla_biomes::PLAINS, &vanilla_biomes::DESERT]);
        // Checkerboard at quart cells decides `(qx >> 4) + (qz >> 4)` mod 2. The search
        // visits columns at x/z multiples of 32 (quart multiples of 8): the first
        // offset whose coarse cell is odd is (-1, +1) → (-32, 32), sampled first at
        // y = 0 (the clamped origin y, from `outFromOrigin(0, -63, 320, 64)`).
        let desert = |biome: BiomeRef| biome.key == vanilla_biomes::DESERT.key;
        let found = source.find_closest_biome_3d(
            BlockPos::new(0, 0, 0),
            MAX_BIOME_SEARCH_RADIUS,
            BIOME_SAMPLE_RESOLUTION_HORIZONTAL,
            BIOME_SAMPLE_RESOLUTION_VERTICAL,
            &desert,
            -64,
            320,
        );
        assert_eq!(found.map(|(pos, _)| pos), Some(BlockPos::new(-32, 0, 32)));
        assert!(found.is_some_and(|(_, biome)| biome.key == vanilla_biomes::DESERT.key));
    }

    #[test]
    fn biome_search_reports_not_found_when_nothing_matches() {
        use steel_registry::{biome::BiomeRef, vanilla_biomes};
        use steel_worldgen::biomes::BiomeSourceKind;

        init_vanilla_registry();
        let source = BiomeSourceKind::fixed(&vanilla_biomes::PLAINS);
        let desert = |biome: BiomeRef| biome.key == vanilla_biomes::DESERT.key;
        assert!(
            source
                .find_closest_biome_3d(
                    BlockPos::new(0, 64, 0),
                    MAX_BIOME_SEARCH_RADIUS,
                    BIOME_SAMPLE_RESOLUTION_HORIZONTAL,
                    BIOME_SAMPLE_RESOLUTION_VERTICAL,
                    &desert,
                    -64,
                    320,
                )
                .is_none()
        );
    }

    #[test]
    fn poi_search_finds_the_closest_matching_record() {
        use crate::poi::poi_storage::PointOfInterestStorage;
        use steel_registry::{REGISTRY, RegistryExt as _, vanilla_poi_types};

        init_vanilla_registry();
        let home_id = REGISTRY
            .poi_types
            .id_from_key(&vanilla_poi_types::HOME.key)
            .expect("home POI type should be registered");
        let mut storage = PointOfInterestStorage::new();
        storage.add(BlockPos::new(20, 70, 0), home_id, 1);
        storage.add(BlockPos::new(-8, 60, 6), home_id, 1);

        let query = PoiTypeOrTag::Type(&vanilla_poi_types::HOME);
        let found = search_poi(&storage, &query, BlockPos::new(0, 64, 0));
        // (-8, 60, 6) is closer in 3D (distSqr 116) than (20, 70, 0) (distSqr 436).
        assert_eq!(
            found.map(|(poi_type, pos)| (poi_type.key.clone(), pos)),
            Some((
                vanilla_poi_types::HOME.key.clone(),
                BlockPos::new(-8, 60, 6)
            ))
        );
    }

    #[test]
    fn poi_search_ignores_records_beyond_the_vanilla_radius() {
        use crate::poi::poi_storage::PointOfInterestStorage;
        use steel_registry::{REGISTRY, RegistryExt as _, vanilla_poi_types};

        init_vanilla_registry();
        let home_id = REGISTRY
            .poi_types
            .id_from_key(&vanilla_poi_types::HOME.key)
            .expect("home POI type should be registered");
        let mut storage = PointOfInterestStorage::new();
        storage.add(BlockPos::new(POI_SEARCH_RADIUS + 1, 64, 0), home_id, 1);

        let query = PoiTypeOrTag::Type(&vanilla_poi_types::HOME);
        assert!(search_poi(&storage, &query, BlockPos::new(0, 64, 0)).is_none());
    }

    #[test]
    fn not_found_errors_use_vanillas_translation_keys() {
        use crate::command::brigadier::CommandSyntaxErrorKind;
        use steel_registry::{vanilla_biomes, vanilla_poi_types};
        use text_components::content::Content;

        init_vanilla_registry();
        let biome_query = BiomeOrTag::Biome(&vanilla_biomes::DESERT);
        let biome_error = biome_not_found(&biome_query);
        let CommandSyntaxErrorKind::Dynamic(component) = biome_error.kind() else {
            panic!("biome miss should be a dynamic command error");
        };
        assert!(matches!(
            &component.content,
            Content::Translate(message) if message.key == "commands.locate.biome.not_found"
        ));

        let poi_query = PoiTypeOrTag::Type(&vanilla_poi_types::HOME);
        let poi_error = poi_not_found(&poi_query);
        let CommandSyntaxErrorKind::Dynamic(component) = poi_error.kind() else {
            panic!("POI miss should be a dynamic command error");
        };
        assert!(matches!(
            &component.content,
            Content::Translate(message) if message.key == "commands.locate.poi.not_found"
        ));
    }
}
