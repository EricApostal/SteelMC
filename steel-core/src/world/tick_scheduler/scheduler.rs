use super::{
    Arc, AtomicI64, AtomicOrdering, BTreeSet, BinaryHeap, BlockPos, BlockRef, BlockTick,
    BoundingBox, ChunkPos, ChunkTickContainer, ChunkTickContainerLifecycle, ChunkTickLists,
    FluidRef, FluidTick, FullChunkRef, FxHashMap, IVec3, Ordering, PackedChunkPos, ScheduledTick,
    ScheduledTickBatch, SectionPos, SyncMutex, SyncRwLock, TickKey, TickList, TickPriority,
    TickSchedulerError, intra_tick_drain_order,
};

pub(crate) struct WorldTickScheduler {
    next_sub_tick_order: AtomicI64,
    phase: SyncRwLock<()>,
    pub(super) state: SyncMutex<WorldTickSchedulerState>,
}

#[derive(Debug, Default)]
pub(super) struct WorldTickSchedulerState {
    pub(super) chunks: FxHashMap<ChunkPos, RegisteredChunkTicks>,
    pub(super) active_block_deadlines: BTreeSet<(i64, PackedChunkPos)>,
    active_fluid_deadlines: BTreeSet<(i64, PackedChunkPos)>,
    pub(super) active_generation: u64,
}

#[derive(Debug)]
pub(super) struct RegisteredChunkTicks {
    pub(super) container: Arc<ChunkTickContainer>,
    pub(super) block_head: Option<i64>,
    pub(super) fluid_head: Option<i64>,
    pub(super) active: Option<ActiveChunkRank>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ActiveChunkRank {
    generation: u64,
    rank: usize,
}

#[derive(Clone, Copy)]
pub(super) enum TickKind {
    Block,
    Fluid,
}

struct ActiveTickContainer {
    pos: ChunkPos,
    rank: usize,
    container: Arc<ChunkTickContainer>,
}

struct TickHeadUpdate {
    pos: ChunkPos,
    trigger_tick: Option<i64>,
}

impl RegisteredChunkTicks {
    const fn head(&self, kind: TickKind) -> Option<i64> {
        match kind {
            TickKind::Block => self.block_head,
            TickKind::Fluid => self.fluid_head,
        }
    }

    const fn set_head(&mut self, kind: TickKind, trigger_tick: Option<i64>) {
        match kind {
            TickKind::Block => self.block_head = trigger_tick,
            TickKind::Fluid => self.fluid_head = trigger_tick,
        }
    }

    pub(super) const fn active_rank(&self, generation: u64) -> Option<usize> {
        match self.active {
            Some(active) if active.generation == generation => Some(active.rank),
            _ => None,
        }
    }
}

impl WorldTickSchedulerState {
    fn advance_active_generation(&mut self) -> u64 {
        let generation = if let Some(generation) = self.active_generation.checked_add(1) {
            generation
        } else {
            for registered in self.chunks.values_mut() {
                registered.active = None;
            }
            1
        };
        self.active_generation = generation;
        generation
    }

    const fn deadlines(&self, kind: TickKind) -> &BTreeSet<(i64, PackedChunkPos)> {
        match kind {
            TickKind::Block => &self.active_block_deadlines,
            TickKind::Fluid => &self.active_fluid_deadlines,
        }
    }

    const fn deadlines_mut(&mut self, kind: TickKind) -> &mut BTreeSet<(i64, PackedChunkPos)> {
        match kind {
            TickKind::Block => &mut self.active_block_deadlines,
            TickKind::Fluid => &mut self.active_fluid_deadlines,
        }
    }

    pub(super) fn set_head(
        &mut self,
        pos: ChunkPos,
        kind: TickKind,
        trigger_tick: Option<i64>,
    ) -> Result<(), TickSchedulerError> {
        let Some(registered) = self.chunks.get(&pos) else {
            return Err(TickSchedulerError::MissingContainer(pos));
        };
        let active = registered.active_rank(self.active_generation).is_some();
        let previous = registered.head(kind);
        if previous == trigger_tick {
            return Ok(());
        }
        let packed = PackedChunkPos::from(pos);
        if active && let Some(previous) = previous {
            assert!(
                self.deadlines_mut(kind).remove(&(previous, packed)),
                "active scheduled-tick head was absent from its deadline index"
            );
        }
        let Some(registered) = self.chunks.get_mut(&pos) else {
            return Err(TickSchedulerError::MissingContainer(pos));
        };
        registered.set_head(kind, trigger_tick);
        if active && let Some(trigger_tick) = trigger_tick {
            assert!(
                self.deadlines_mut(kind).insert((trigger_tick, packed)),
                "active scheduled-tick deadline was already indexed"
            );
        }
        Ok(())
    }

    fn take_due(&mut self, kind: TickKind, current_tick: i64) -> Vec<ActiveTickContainer> {
        let mut due = Vec::new();
        while let Some((trigger_tick, packed)) = self.deadlines(kind).first().copied() {
            if trigger_tick > current_tick {
                break;
            }
            assert_eq!(
                self.deadlines_mut(kind).pop_first(),
                Some((trigger_tick, packed)),
                "due scheduled-tick deadline changed during collection"
            );
            let pos = packed.to_chunk_pos();
            let Some(registered) = self.chunks.get_mut(&pos) else {
                panic!("active scheduled-tick deadline lost its registered container");
            };
            assert_eq!(
                registered.head(kind),
                Some(trigger_tick),
                "active scheduled-tick deadline diverged from its container head"
            );
            let Some(rank) = registered.active_rank(self.active_generation) else {
                panic!("inactive scheduled-tick container retained an active deadline");
            };
            due.push(ActiveTickContainer {
                pos,
                rank,
                container: Arc::clone(&registered.container),
            });
            registered.set_head(kind, None);
        }
        due
    }
}

impl WorldTickScheduler {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            next_sub_tick_order: AtomicI64::new(0),
            phase: SyncRwLock::new(()),
            state: SyncMutex::new(WorldTickSchedulerState::default()),
        }
    }

    /// Allocates the world-global order before container lookup or deduplication.
    ///
    /// Vanilla creates the `ScheduledTick` before asking `LevelTicks` to store
    /// it, so failed and duplicate scheduling attempts consume an order too.
    pub(crate) fn next_sub_tick_order(&self) -> i64 {
        self.next_sub_tick_order
            .fetch_add(1, AtomicOrdering::Relaxed)
    }

    /// Registers an unpublished Full chunk's stable queues with the world index.
    pub(crate) fn register_chunk(&self, chunk: FullChunkRef<'_>) -> Result<(), TickSchedulerError> {
        let _phase = self.phase.read();
        let container = chunk.scheduled_tick_container();
        let mut container_state = container.state.lock();
        if container_state.lifecycle != ChunkTickContainerLifecycle::PrePublication {
            return Err(TickSchedulerError::AlreadyRegistered(chunk.common().pos));
        }
        let block_head = container_state
            .lists
            .block()
            .peek()
            .map(|tick| tick.trigger_tick);
        let fluid_head = container_state
            .lists
            .fluid()
            .peek()
            .map(|tick| tick.trigger_tick);
        let mut state = self.state.lock();
        if state.chunks.contains_key(&chunk.common().pos) {
            return Err(TickSchedulerError::AlreadyRegistered(chunk.common().pos));
        }
        state.chunks.insert(
            chunk.common().pos,
            RegisteredChunkTicks {
                container: Arc::clone(container),
                block_head,
                fluid_head,
                active: None,
            },
        );
        container_state.lifecycle = ChunkTickContainerLifecycle::Registered;
        Ok(())
    }

    /// Anchors saved/proto delays when a Full chunk first becomes block-ticking.
    ///
    /// `TickList::unpack` is idempotent, so later readiness promotions preserve
    /// the original absolute deadlines.
    pub(crate) fn unpack_chunk(
        &self,
        pos: ChunkPos,
        current_tick: i64,
    ) -> Result<(), TickSchedulerError> {
        let _phase = self.phase.read();
        let container = self
            .state
            .lock()
            .chunks
            .get(&pos)
            .map(|registered| Arc::clone(&registered.container))
            .ok_or(TickSchedulerError::MissingContainer(pos))?;
        let mut container_state = container.state.lock();
        if container_state.lifecycle != ChunkTickContainerLifecycle::Registered {
            return Err(TickSchedulerError::MissingContainer(pos));
        }
        container_state.lists.block_mut().unpack(current_tick);
        container_state.lists.fluid_mut().unpack(current_tick);
        let block_head = container_state
            .lists
            .block()
            .peek()
            .map(|tick| tick.trigger_tick);
        let fluid_head = container_state
            .lists
            .fluid()
            .peek()
            .map(|tick| tick.trigger_tick);
        let mut state = self.state.lock();
        let Some(registered) = state.chunks.get(&pos) else {
            return Err(TickSchedulerError::MissingContainer(pos));
        };
        if !Arc::ptr_eq(&registered.container, &container) {
            return Err(TickSchedulerError::ContainerMismatch(pos));
        }
        state.set_head(pos, TickKind::Block, block_head)?;
        state.set_head(pos, TickKind::Fluid, fluid_head)?;
        Ok(())
    }

    /// Removes a finally-unloaded Full chunk after its last save completed.
    pub(crate) fn unregister_chunk(&self, pos: ChunkPos) {
        let _phase = self.phase.write();
        let registered = {
            let mut state = self.state.lock();
            let active_generation = state.active_generation;
            let registered = state.chunks.remove(&pos);
            if let Some(registered) = &registered
                && registered.active_rank(active_generation).is_some()
            {
                let packed = PackedChunkPos::from(pos);
                if let Some(head) = registered.block_head {
                    assert!(
                        state.active_block_deadlines.remove(&(head, packed)),
                        "unloaded active block-tick head was absent from its deadline index"
                    );
                }
                if let Some(head) = registered.fluid_head {
                    assert!(
                        state.active_fluid_deadlines.remove(&(head, packed)),
                        "unloaded active fluid-tick head was absent from its deadline index"
                    );
                }
            }
            registered
        };
        if let Some(registered) = registered {
            registered.container.state.lock().lifecycle = ChunkTickContainerLifecycle::Finalized;
        }
    }

    #[cfg(test)]
    pub(crate) fn has_registered_chunk(&self, pos: ChunkPos) -> bool {
        self.state.lock().chunks.contains_key(&pos)
    }

    #[cfg(test)]
    pub(crate) fn has_indexed_head(&self, pos: ChunkPos) -> bool {
        let state = self.state.lock();
        state.chunks.get(&pos).is_some_and(|registered| {
            registered.block_head.is_some() || registered.fluid_head.is_some()
        })
    }

    /// Reconciles the active sparse deadline index at a rare ticking-snapshot rebuild.
    pub(crate) fn reconcile_active_chunks<I>(
        &self,
        active_chunks: I,
    ) -> Result<(), TickSchedulerError>
    where
        I: Iterator<Item = ChunkPos> + Clone,
    {
        let _phase = self.phase.write();
        let mut state = self.state.lock();
        for pos in active_chunks.clone() {
            if !state.chunks.contains_key(&pos) {
                return Err(TickSchedulerError::MissingContainer(pos));
            }
        }
        state.active_block_deadlines.clear();
        state.active_fluid_deadlines.clear();
        let generation = state.advance_active_generation();
        for (rank, pos) in active_chunks.enumerate() {
            let (block_head, fluid_head) = {
                let Some(registered) = state.chunks.get_mut(&pos) else {
                    return Err(TickSchedulerError::MissingContainer(pos));
                };
                assert!(
                    registered
                        .active
                        .is_none_or(|active| active.generation != generation),
                    "active scheduled-tick chunk appeared twice during reconciliation"
                );
                registered.active = Some(ActiveChunkRank { generation, rank });
                (registered.block_head, registered.fluid_head)
            };
            let packed = PackedChunkPos::from(pos);
            if let Some(block_head) = block_head {
                assert!(
                    state.active_block_deadlines.insert((block_head, packed)),
                    "active block-tick chunk appeared twice during reconciliation"
                );
            }
            if let Some(fluid_head) = fluid_head {
                assert!(
                    state.active_fluid_deadlines.insert((fluid_head, packed)),
                    "active fluid-tick chunk appeared twice during reconciliation"
                );
            }
        }
        Ok(())
    }

    pub(crate) fn schedule_block(
        &self,
        chunk: FullChunkRef<'_>,
        block: BlockRef,
        pos: BlockPos,
        trigger_tick: i64,
        priority: TickPriority,
        sub_tick_order: i64,
    ) -> Result<bool, TickSchedulerError> {
        let _phase = self.phase.read();
        let container = chunk.scheduled_tick_container();
        let mut container_state = container.state.lock();
        if container_state.lifecycle == ChunkTickContainerLifecycle::PrePublication {
            return Ok(container_state.lists.block_mut().schedule(
                block,
                pos,
                trigger_tick,
                priority,
                sub_tick_order,
            ));
        }
        if container_state.lifecycle == ChunkTickContainerLifecycle::Finalized {
            return Err(TickSchedulerError::MissingContainer(chunk.common().pos));
        }
        let previous_head = container_state
            .lists
            .block()
            .peek()
            .map(|tick| tick.trigger_tick);
        let added = container_state.lists.block_mut().schedule(
            block,
            pos,
            trigger_tick,
            priority,
            sub_tick_order,
        );
        if !added {
            return Ok(false);
        }
        let head = container_state
            .lists
            .block()
            .peek()
            .map(|tick| tick.trigger_tick);
        if head == previous_head {
            return Ok(true);
        }
        let mut state = self.state.lock();
        let Some(registered) = state.chunks.get(&chunk.common().pos) else {
            return Err(TickSchedulerError::MissingContainer(chunk.common().pos));
        };
        if !Arc::ptr_eq(&registered.container, container) {
            return Err(TickSchedulerError::ContainerMismatch(chunk.common().pos));
        }
        state.set_head(chunk.common().pos, TickKind::Block, head)?;
        Ok(true)
    }

    pub(crate) fn schedule_fluid(
        &self,
        chunk: FullChunkRef<'_>,
        fluid: FluidRef,
        pos: BlockPos,
        trigger_tick: i64,
        priority: TickPriority,
        sub_tick_order: i64,
    ) -> Result<bool, TickSchedulerError> {
        let _phase = self.phase.read();
        let container = chunk.scheduled_tick_container();
        let mut container_state = container.state.lock();
        if container_state.lifecycle == ChunkTickContainerLifecycle::PrePublication {
            return Ok(container_state.lists.fluid_mut().schedule(
                fluid,
                pos,
                trigger_tick,
                priority,
                sub_tick_order,
            ));
        }
        if container_state.lifecycle == ChunkTickContainerLifecycle::Finalized {
            return Err(TickSchedulerError::MissingContainer(chunk.common().pos));
        }
        let previous_head = container_state
            .lists
            .fluid()
            .peek()
            .map(|tick| tick.trigger_tick);
        let added = container_state.lists.fluid_mut().schedule(
            fluid,
            pos,
            trigger_tick,
            priority,
            sub_tick_order,
        );
        if !added {
            return Ok(false);
        }
        let head = container_state
            .lists
            .fluid()
            .peek()
            .map(|tick| tick.trigger_tick);
        if head == previous_head {
            return Ok(true);
        }
        let mut state = self.state.lock();
        let Some(registered) = state.chunks.get(&chunk.common().pos) else {
            return Err(TickSchedulerError::MissingContainer(chunk.common().pos));
        };
        if !Arc::ptr_eq(&registered.container, container) {
            return Err(TickSchedulerError::ContainerMismatch(chunk.common().pos));
        }
        state.set_head(chunk.common().pos, TickKind::Fluid, head)?;
        Ok(true)
    }

    /// Selects the ready block batch from sparse live-container heads.
    pub(crate) fn begin_tick(
        &self,
        current_tick: i64,
        max_ticks: usize,
    ) -> ScheduledTickBatch<BlockRef> {
        self.collect_ticks(
            TickKind::Block,
            ChunkTickLists::block_mut,
            current_tick,
            max_ticks,
        )
    }

    /// Removes every scheduled block and fluid tick whose position is inside `area`.
    ///
    /// Mirrors vanilla `LevelTicks.clearArea`: only chunk containers overlapping the area's
    /// chunk range are visited, and each container's head/deadline index entry is repaired
    /// exactly when its head changes. Vanilla's callers only clear block ticks; Steel's single
    /// scheduler clears the fluid queue too, so a cleared area cannot leave fluid ticks behind.
    /// Ticks already collected into an executor batch keep running: Steel hands those to the
    /// executor as an immutable batch, so it has no mutable `toRunThisTick` queue to purge.
    pub(crate) fn clear_area(&self, area: BoundingBox) {
        let _phase = self.phase.read();
        for (pos, container) in self.containers_in_area(area) {
            let mut container_state = container.state.lock();
            // The container lock is held across the head repair, matching `schedule_block`:
            // otherwise a concurrent schedule could slip a tick in between and leave a stale
            // deadline behind.
            let mut state = self.state.lock();
            if container_state.lifecycle != ChunkTickContainerLifecycle::Registered {
                continue;
            }
            let Some(registered) = state.chunks.get(&pos) else {
                continue;
            };
            if !Arc::ptr_eq(&registered.container, &container) {
                continue;
            }
            clear_container_area(&mut container_state.lists, area);
            let block_head = container_state
                .lists
                .block()
                .peek()
                .map(|tick| tick.trigger_tick);
            let fluid_head = container_state
                .lists
                .fluid()
                .peek()
                .map(|tick| tick.trigger_tick);
            // `set_head` repairs the per-kind deadline index exactly like vanilla's
            // `updateContainerScheduling`, and drops the entry when the container is empty.
            if let Err(error) = state.set_head(pos, TickKind::Block, block_head) {
                panic!("scheduled-tick head index invariant failed: {error:?}");
            }
            if let Err(error) = state.set_head(pos, TickKind::Fluid, fluid_head) {
                panic!("scheduled-tick head index invariant failed: {error:?}");
            }
        }
    }

    /// Copies this scheduler's ticks inside `area`, re-scheduled at `offset` positions.
    ///
    /// Mirrors vanilla `LevelTicks.copyArea`, which delegates to `copyAreaFrom` with itself.
    pub(crate) fn copy_area(&self, area: BoundingBox, offset: IVec3) {
        self.copy_area_from(self, area, offset);
    }

    /// Copies `source`'s ticks inside `area`, re-scheduled at `offset` positions.
    ///
    /// Mirrors vanilla `LevelTicks.copyAreaFrom`: every collected tick is re-scheduled as
    /// `ScheduledTick` at `pos + offset` with `sub_tick_order - min + max + 1`, preserving
    /// the trigger tick and priority. Sub-tick statistics are computed per tick kind, matching
    /// vanilla's per-type `LevelTicks` instances. Ticks whose offset position has no registered
    /// container are dropped, like vanilla's unloaded-position drop in `schedule`.
    pub(crate) fn copy_area_from(
        &self,
        source: &WorldTickScheduler,
        area: BoundingBox,
        offset: IVec3,
    ) {
        let (block_ticks, fluid_ticks) = source.collect_area_ticks(area);
        copy_kind_ticks(
            self,
            block_ticks,
            offset,
            TickKind::Block,
            ChunkTickLists::block_mut,
        );
        copy_kind_ticks(
            self,
            fluid_ticks,
            offset,
            TickKind::Fluid,
            ChunkTickLists::fluid_mut,
        );
    }

    /// Snapshots the chunk containers registered for the chunk range overlapping `area`.
    ///
    /// Mirrors vanilla's `forContainersInArea`: the block min/max corners convert to chunk
    /// coordinates via `SectionPos` (`>> 4`), so partially overlapped edge chunks are included.
    fn containers_in_area(&self, area: BoundingBox) -> Vec<(ChunkPos, Arc<ChunkTickContainer>)> {
        let state = self.state.lock();
        let mut containers = Vec::new();
        for chunk_x in SectionPos::block_to_section_coord(area.min_x())
            ..=SectionPos::block_to_section_coord(area.max_x())
        {
            for chunk_z in SectionPos::block_to_section_coord(area.min_z())
                ..=SectionPos::block_to_section_coord(area.max_z())
            {
                let pos = ChunkPos::new(chunk_x, chunk_z);
                if let Some(registered) = state.chunks.get(&pos) {
                    containers.push((pos, Arc::clone(&registered.container)));
                }
            }
        }
        containers
    }

    /// Collects every queued block and fluid tick inside `area`.
    ///
    /// Mirrors vanilla's `getAll` stream over the live queues. Not-yet-unpacked saved entries are
    /// skipped: they carry a relative delay but no sub-tick order for the copy formula, and
    /// vanilla's live world unpacks them before a container joins `LevelTicks`.
    fn collect_area_ticks(&self, area: BoundingBox) -> (Vec<BlockTick>, Vec<FluidTick>) {
        let _phase = self.phase.read();
        let mut block_ticks = Vec::new();
        let mut fluid_ticks = Vec::new();
        for (_, container) in self.containers_in_area(area) {
            let container_state = container.state.lock();
            block_ticks.extend(
                container_state
                    .lists
                    .block()
                    .live_entries()
                    .filter(|tick| area.contains_blockpos(tick.pos)),
            );
            fluid_ticks.extend(
                container_state
                    .lists
                    .fluid()
                    .live_entries()
                    .filter(|tick| area.contains_blockpos(tick.pos)),
            );
        }
        (block_ticks, fluid_ticks)
    }

    /// Inserts one area-copied tick, dropping it when its chunk is not registered.
    ///
    /// Mirrors vanilla `LevelTicks.schedule` plus the `chunkScheduleUpdater` head repair:
    /// a destination container that already schedules the same `(pos, type)` ignores the copy.
    fn schedule_copied_tick<T: TickKey>(
        &self,
        kind: TickKind,
        select: fn(&mut ChunkTickLists) -> &mut TickList<T>,
        tick: ScheduledTick<T>,
    ) {
        let _phase = self.phase.read();
        let pos = ChunkPos::from_block_pos(tick.pos);
        let container = {
            let state = self.state.lock();
            let Some(registered) = state.chunks.get(&pos) else {
                return;
            };
            Arc::clone(&registered.container)
        };
        let mut container_state = container.state.lock();
        if container_state.lifecycle != ChunkTickContainerLifecycle::Registered {
            return;
        }
        if !select(&mut container_state.lists).schedule(
            tick.tick_type,
            tick.pos,
            tick.trigger_tick,
            tick.priority,
            tick.sub_tick_order,
        ) {
            return;
        }
        let head = select(&mut container_state.lists)
            .peek()
            .map(|tick| tick.trigger_tick);
        // Held across the metadata update like `schedule_block`, so the indexed head cannot go
        // stale between reading it and repairing the deadline set.
        let mut state = self.state.lock();
        if state
            .chunks
            .get(&pos)
            .is_none_or(|registered| !Arc::ptr_eq(&registered.container, &container))
        {
            return;
        }
        if let Err(error) = state.set_head(pos, kind, head) {
            panic!("scheduled-tick head index invariant failed: {error:?}");
        }
    }

    /// Selects fluids after block callbacks using the same captured game time.
    pub(crate) fn collect_fluid_ticks(
        &self,
        current_tick: i64,
        max_ticks: usize,
    ) -> ScheduledTickBatch<FluidRef> {
        self.collect_ticks(
            TickKind::Fluid,
            ChunkTickLists::fluid_mut,
            current_tick,
            max_ticks,
        )
    }

    fn collect_ticks<T: TickKey>(
        &self,
        kind: TickKind,
        select: fn(&mut ChunkTickLists) -> &mut TickList<T>,
        current_tick: i64,
        max_ticks: usize,
    ) -> ScheduledTickBatch<T> {
        if max_ticks == 0 {
            return ScheduledTickBatch {
                ticks: Vec::new(),
                changed_containers: Vec::new(),
            };
        }
        let _phase = self.phase.write();
        let due = self.state.lock().take_due(kind, current_tick);
        if due.is_empty() {
            return ScheduledTickBatch {
                ticks: Vec::new(),
                changed_containers: Vec::new(),
            };
        }
        let (batch, head_updates) = collect_registered_ticks(due, select, current_tick, max_ticks);
        let mut state = self.state.lock();
        for update in head_updates {
            if let Err(error) = state.set_head(update.pos, kind, update.trigger_tick) {
                panic!("scheduled-tick head index invariant failed: {error:?}");
            }
        }
        batch
    }
}

impl Default for WorldTickScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct ReadyContainer {
    pos: ChunkPos,
    rank: usize,
    container: Arc<ChunkTickContainer>,
    priority: TickPriority,
    sub_tick_order: i64,
    dirty_reported: bool,
}

impl PartialEq for ReadyContainer {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
            && self.sub_tick_order == other.sub_tick_order
            && self.rank == other.rank
    }
}

impl Eq for ReadyContainer {}

impl ReadyContainer {
    fn new<T: TickKey>(
        active: ActiveTickContainer,
        tick: ScheduledTick<T>,
        dirty_reported: bool,
    ) -> Self {
        Self {
            pos: active.pos,
            rank: active.rank,
            container: active.container,
            priority: tick.priority,
            sub_tick_order: tick.sub_tick_order,
            dirty_reported,
        }
    }

    const fn refresh<T: TickKey>(&mut self, tick: ScheduledTick<T>) {
        self.priority = tick.priority;
        self.sub_tick_order = tick.sub_tick_order;
    }
}

impl PartialOrd for ReadyContainer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ReadyContainer {
    fn cmp(&self, other: &Self) -> Ordering {
        intra_tick_drain_order(
            self.priority,
            self.sub_tick_order,
            other.priority,
            other.sub_tick_order,
        )
        .reverse()
        .then_with(|| other.rank.cmp(&self.rank))
    }
}

fn prepare_ready_containers<T: TickKey>(
    due_containers: Vec<ActiveTickContainer>,
    select: fn(&mut ChunkTickLists) -> &mut TickList<T>,
    current_tick: i64,
) -> (BinaryHeap<ReadyContainer>, Vec<TickHeadUpdate>) {
    let mut ready_containers = BinaryHeap::with_capacity(due_containers.len());
    let mut head_updates = Vec::with_capacity(due_containers.len());
    for active in due_containers {
        let head = {
            let mut state = active.container.state.lock();
            assert_eq!(
                state.lifecycle,
                ChunkTickContainerLifecycle::Registered,
                "active scheduled-tick container was not registered"
            );
            select(&mut state.lists).peek()
        };
        if let Some(tick) = head
            && tick.trigger_tick <= current_tick
        {
            ready_containers.push(ReadyContainer::new(active, tick, false));
        } else {
            head_updates.push(TickHeadUpdate {
                pos: active.pos,
                trigger_tick: head.map(|tick| tick.trigger_tick),
            });
        }
    }
    (ready_containers, head_updates)
}

/// Re-schedules copied ticks into `target` with vanilla's sub-tick renumbering.
///
/// Vanilla's `copyAreaFrom` renumbers `subTickOrder` as `order - min + max + 1` over the
/// collected set, so copied ticks keep their relative intra-tick order but sort after every
/// tick already queued at the destination. An empty set schedules nothing, exactly like
/// `LongSummaryStatistics` over an empty stream feeding an empty `forEach`.
fn copy_kind_ticks<T: TickKey>(
    target: &WorldTickScheduler,
    ticks: Vec<ScheduledTick<T>>,
    offset: IVec3,
    kind: TickKind,
    select: fn(&mut ChunkTickLists) -> &mut TickList<T>,
) {
    let Some(first) = ticks.first() else {
        return;
    };
    let mut min_sub_tick = first.sub_tick_order;
    let mut max_sub_tick = first.sub_tick_order;
    for tick in &ticks {
        min_sub_tick = min_sub_tick.min(tick.sub_tick_order);
        max_sub_tick = max_sub_tick.max(tick.sub_tick_order);
    }
    for tick in ticks {
        let shifted = ScheduledTick {
            pos: tick.pos.offset(offset.x, offset.y, offset.z),
            sub_tick_order: tick
                .sub_tick_order
                .wrapping_sub(min_sub_tick)
                .wrapping_add(max_sub_tick)
                .wrapping_add(1),
            ..tick
        };
        target.schedule_copied_tick(kind, select, shifted);
    }
}

/// Removes every queued block and fluid tick inside `area`, live and not yet unpacked.
///
/// Mirrors the per-container half of vanilla `LevelTicks.clearArea`: live ticks via
/// `LevelChunkTicks.removeIf`, and Steel additionally purges not-yet-unpacked saved ticks
/// so pre-ticking containers cannot resurrect cleared entries on `unpack`.
fn clear_container_area(lists: &mut ChunkTickLists, area: BoundingBox) {
    lists
        .block_mut()
        .remove_live_matching(|tick| area.contains_blockpos(tick.pos));
    lists
        .block_mut()
        .remove_pending_matching(|tick| area.contains_blockpos(tick.pos));
    lists
        .fluid_mut()
        .remove_live_matching(|tick| area.contains_blockpos(tick.pos));
    lists
        .fluid_mut()
        .remove_pending_matching(|tick| area.contains_blockpos(tick.pos));
}

/// Selects at most `max_ticks` ready entries from sparse live-container heads.
///
/// Only queue heads compete globally. Revealing the next head after each pop is
/// what preserves Vanilla's per-chunk deadline ordering when several ticks are
/// already overdue.
fn collect_registered_ticks<T: TickKey>(
    due_containers: Vec<ActiveTickContainer>,
    select: fn(&mut ChunkTickLists) -> &mut TickList<T>,
    current_tick: i64,
    max_ticks: usize,
) -> (ScheduledTickBatch<T>, Vec<TickHeadUpdate>) {
    let (mut ready_containers, mut head_updates) =
        prepare_ready_containers(due_containers, select, current_tick);

    let mut ticks = Vec::with_capacity(max_ticks.min(ready_containers.len()));
    let mut changed_containers = Vec::with_capacity(ready_containers.len());
    while ticks.len() < max_ticks {
        let Some(mut ready_container) = ready_containers.pop() else {
            break;
        };
        let mut container_state = ready_container.container.state.lock();
        assert_eq!(
            container_state.lifecycle,
            ChunkTickContainerLifecycle::Registered,
            "ready scheduled-tick container was not registered"
        );
        let container = select(&mut container_state.lists);
        let Some(tick) = container.pop_ready(current_tick) else {
            head_updates.push(TickHeadUpdate {
                pos: ready_container.pos,
                trigger_tick: container.peek().map(|tick| tick.trigger_tick),
            });
            continue;
        };

        if !ready_container.dirty_reported {
            changed_containers.push(ready_container.rank);
            ready_container.dirty_reported = true;
        }
        ticks.push(tick);

        // Vanilla keeps draining the current container while its next head is
        // no later in intra-tick order than the best competing container. In
        // particular, an exact tie stays with the current container.
        let next_competing_container = ready_containers
            .peek()
            .map(|competitor| (competitor.priority, competitor.sub_tick_order));
        while ticks.len() < max_ticks {
            let Some(next_tick) = container.peek_ready(current_tick) else {
                break;
            };
            if next_competing_container.is_some_and(|(priority, sub_tick_order)| {
                intra_tick_drain_order(
                    next_tick.priority,
                    next_tick.sub_tick_order,
                    priority,
                    sub_tick_order,
                ) == Ordering::Greater
            }) {
                break;
            }
            let Some(next_tick) = container.pop_ready(current_tick) else {
                break;
            };
            ticks.push(next_tick);
        }

        let next_tick = container.peek();
        drop(container_state);
        if let Some(next_tick) = next_tick {
            if ticks.len() < max_ticks && next_tick.trigger_tick <= current_tick {
                ready_container.refresh(next_tick);
                ready_containers.push(ready_container);
            } else {
                head_updates.push(TickHeadUpdate {
                    pos: ready_container.pos,
                    trigger_tick: Some(next_tick.trigger_tick),
                });
            }
        } else {
            head_updates.push(TickHeadUpdate {
                pos: ready_container.pos,
                trigger_tick: None,
            });
        }
    }

    for ready_container in ready_containers {
        let next_tick = {
            let mut state = ready_container.container.state.lock();
            select(&mut state.lists).peek()
        };
        head_updates.push(TickHeadUpdate {
            pos: ready_container.pos,
            trigger_tick: next_tick.map(|tick| tick.trigger_tick),
        });
    }

    (
        ScheduledTickBatch {
            ticks,
            changed_containers,
        },
        head_updates,
    )
}
