//! Mob control state.

use glam::DVec3;
use std::f32::consts::PI;
use steel_math::wrap_degrees;
use steel_registry::vanilla_attributes;

use crate::entity::{LivingTravelInput, Mob};

pub(crate) const DEFAULT_LOOK_Y_MAX_ROT_SPEED: f32 = 10.0;
pub(crate) const DEFAULT_LOOK_X_MAX_ROT_ANGLE: f32 = 40.0;
const HEAD_STABLE_ANGLE: f32 = 15.0;
const DELAY_UNTIL_STARTING_TO_FACE_FORWARD: i32 = 10;
const HOW_LONG_IT_TAKES_TO_FACE_FORWARD: f32 = 10.0;

pub(crate) fn rotate_towards(from_angle: f32, to_angle: f32, max_rot: f32) -> f32 {
    let diff = wrap_degrees(to_angle - from_angle);
    let diff_clamped = diff.clamp(-max_rot, max_rot);
    from_angle + diff_clamped
}

pub(crate) fn rotate_if_necessary(base_angle: f32, target_angle: f32, max_angle_diff: f32) -> f32 {
    let delta_angle = wrap_degrees(target_angle - base_angle);
    let delta_angle_clamped = delta_angle.clamp(-max_angle_diff, max_angle_diff);
    target_angle - delta_angle_clamped
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveControlOperation {
    Wait,
    MoveTo,
    Strafe,
    Jumping,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveControl {
    wanted_position: DVec3,
    speed_modifier: f64,
    strafe_forward: f32,
    strafe_right: f32,
    operation: MoveControlOperation,
}

impl MoveControl {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            wanted_position: DVec3::ZERO,
            speed_modifier: 0.0,
            strafe_forward: 0.0,
            strafe_right: 0.0,
            operation: MoveControlOperation::Wait,
        }
    }

    #[must_use]
    pub const fn operation(&self) -> MoveControlOperation {
        self.operation
    }

    #[must_use]
    pub const fn wanted_position(&self) -> DVec3 {
        self.wanted_position
    }

    #[must_use]
    pub const fn speed_modifier(&self) -> f64 {
        self.speed_modifier
    }

    #[must_use]
    pub const fn strafe_forward(&self) -> f32 {
        self.strafe_forward
    }

    #[must_use]
    pub const fn strafe_right(&self) -> f32 {
        self.strafe_right
    }

    pub fn set_wanted_position(&mut self, position: DVec3, speed_modifier: f64) {
        self.wanted_position = position;
        self.speed_modifier = speed_modifier;
        if self.operation != MoveControlOperation::Jumping {
            self.operation = MoveControlOperation::MoveTo;
        }
    }

    pub const fn strafe(&mut self, forward: f32, right: f32) {
        self.operation = MoveControlOperation::Strafe;
        self.strafe_forward = forward;
        self.strafe_right = right;
        self.speed_modifier = 0.25;
    }

    pub const fn set_wait(&mut self) {
        self.operation = MoveControlOperation::Wait;
    }

    pub const fn set_jumping(&mut self) {
        self.operation = MoveControlOperation::Jumping;
    }
}

impl Default for MoveControl {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JumpControl {
    jump: bool,
}

impl JumpControl {
    #[must_use]
    pub const fn new() -> Self {
        Self { jump: false }
    }

    pub const fn jump(&mut self) {
        self.jump = true;
    }

    pub const fn tick(&mut self) -> bool {
        let jump = self.jump;
        self.jump = false;
        jump
    }
}

impl Default for JumpControl {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookControl {
    wanted_position: DVec3,
    y_max_rot_speed: f32,
    x_max_rot_angle: f32,
    look_at_cooldown: i32,
}

impl LookControl {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            wanted_position: DVec3::ZERO,
            y_max_rot_speed: DEFAULT_LOOK_Y_MAX_ROT_SPEED,
            x_max_rot_angle: DEFAULT_LOOK_X_MAX_ROT_ANGLE,
            look_at_cooldown: 0,
        }
    }

    #[must_use]
    pub const fn wanted_position(&self) -> DVec3 {
        self.wanted_position
    }

    #[must_use]
    pub const fn y_max_rot_speed(&self) -> f32 {
        self.y_max_rot_speed
    }

    #[must_use]
    pub const fn x_max_rot_angle(&self) -> f32 {
        self.x_max_rot_angle
    }

    #[must_use]
    pub const fn is_looking_at_target(&self) -> bool {
        self.look_at_cooldown > 0
    }

    pub const fn set_look_at(
        &mut self,
        position: DVec3,
        y_max_rot_speed: f32,
        x_max_rot_angle: f32,
    ) {
        self.wanted_position = position;
        self.y_max_rot_speed = y_max_rot_speed;
        self.x_max_rot_angle = x_max_rot_angle;
        self.look_at_cooldown = 2;
    }

    pub const fn tick_cooldown(&mut self) -> bool {
        if self.look_at_cooldown <= 0 {
            return false;
        }

        self.look_at_cooldown -= 1;
        true
    }
}

impl Default for LookControl {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyRotationInput {
    moving: bool,
    carrying_mob_passenger: bool,
    y_rot: f32,
    y_body_rot: f32,
    y_head_rot: f32,
    max_head_y_rot: f32,
}

impl BodyRotationInput {
    #[must_use]
    pub const fn new(
        moving: bool,
        carrying_mob_passenger: bool,
        y_rot: f32,
        y_body_rot: f32,
        y_head_rot: f32,
        max_head_y_rot: f32,
    ) -> Self {
        Self {
            moving,
            carrying_mob_passenger,
            y_rot,
            y_body_rot,
            y_head_rot,
            max_head_y_rot,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyRotationUpdate {
    y_body_rot: f32,
    y_head_rot: f32,
}

impl BodyRotationUpdate {
    #[must_use]
    pub const fn y_body_rot(self) -> f32 {
        self.y_body_rot
    }

    #[must_use]
    pub const fn y_head_rot(self) -> f32 {
        self.y_head_rot
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyRotationControl {
    head_stable_time: i32,
    last_stable_y_head_rot: f32,
}

impl BodyRotationControl {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            head_stable_time: 0,
            last_stable_y_head_rot: 0.0,
        }
    }

    #[must_use]
    pub const fn head_stable_time(self) -> i32 {
        self.head_stable_time
    }

    #[must_use]
    pub const fn last_stable_y_head_rot(self) -> f32 {
        self.last_stable_y_head_rot
    }

    pub fn tick(&mut self, input: BodyRotationInput) -> BodyRotationUpdate {
        let mut y_body_rot = input.y_body_rot;
        let mut y_head_rot = input.y_head_rot;

        if input.moving {
            y_body_rot = input.y_rot;
            y_head_rot = rotate_if_necessary(y_head_rot, y_body_rot, input.max_head_y_rot);
            self.last_stable_y_head_rot = y_head_rot;
            self.head_stable_time = 0;
        } else if !input.carrying_mob_passenger {
            if (y_head_rot - self.last_stable_y_head_rot).abs() > HEAD_STABLE_ANGLE {
                self.head_stable_time = 0;
                self.last_stable_y_head_rot = y_head_rot;
                y_body_rot = rotate_if_necessary(y_body_rot, y_head_rot, input.max_head_y_rot);
            } else {
                self.head_stable_time += 1;
                if self.head_stable_time > DELAY_UNTIL_STARTING_TO_FACE_FORWARD {
                    let time_since_starting_to_face_forward =
                        self.head_stable_time - DELAY_UNTIL_STARTING_TO_FACE_FORWARD;
                    let face_forward_fraction = (time_since_starting_to_face_forward as f32
                        / HOW_LONG_IT_TAKES_TO_FACE_FORWARD)
                        .clamp(0.0, 1.0);
                    let angle_remaining_until_facing_forward =
                        input.max_head_y_rot * (1.0 - face_forward_fraction);
                    y_body_rot = rotate_if_necessary(
                        y_body_rot,
                        y_head_rot,
                        angle_remaining_until_facing_forward,
                    );
                }
            }
        }

        BodyRotationUpdate {
            y_body_rot,
            y_head_rot,
        }
    }
}

impl Default for BodyRotationControl {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct MobControls {
    pub move_control: MoveControl,
    pub jump_control: JumpControl,
    pub look_control: LookControl,
    pub body_rotation_control: BodyRotationControl,
}

impl MobControls {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            move_control: MoveControl::new(),
            jump_control: JumpControl::new(),
            look_control: LookControl::new(),
            body_rotation_control: BodyRotationControl::new(),
        }
    }
}

/// Vanilla `FlyingMoveControl` input: the movement state the shared mover reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyingMoveControlInput {
    /// Vanilla `MoveControl.Operation`.
    pub operation: MoveControlOperation,
    /// Vanilla `wantedX/Y/Z`.
    pub wanted_position: DVec3,
    /// Vanilla `mob.getX/Y/Z()`.
    pub position: DVec3,
    /// Vanilla `mob.getYRot()`.
    pub y_rot: f32,
    /// Vanilla `mob.getXRot()`.
    pub x_rot: f32,
    /// Vanilla `mob.onGround()`.
    pub on_ground: bool,
    /// Vanilla `speedModifier` (`MoveControl.speedModifier`).
    pub speed_modifier: f64,
    /// Vanilla `Attributes.MOVEMENT_SPEED`.
    pub movement_speed: f64,
    /// Vanilla `Attributes.FLYING_SPEED`.
    pub flying_speed: f64,
}

/// Vanilla `FlyingMoveControl` output: the writes vanilla performs on the mob.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyingMoveControlOutput {
    /// Vanilla `mob.setYRot`.
    pub y_rot: f32,
    /// Vanilla `mob.setXRot`.
    pub x_rot: f32,
    /// Vanilla `mob.setSpeed`; vanilla's `Mob.setSpeed` also writes `zza`.
    pub speed: f32,
    /// Vanilla `mob.setYya`.
    pub yya: f32,
    /// Vanilla `mob.setZza`.
    ///
    /// Equal to [`Self::speed`] on the `MOVE_TO` path (vanilla writes it through
    /// `Mob.setSpeed`) and `0.0` while idling, so a caller that maps this onto the
    /// forward travel input keeps the mob translating instead of spinning in place.
    pub zza: f32,
    /// Vanilla `mob.setNoGravity`.
    pub no_gravity: bool,
}

/// Vanilla `FlyingMoveControl<T extends Mob>`: turns toward the wanted point on the
/// horizontal plane and pitches toward it within `max_turn` degrees per tick, using
/// the flying-speed attribute while airborne.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlyingMoveControl {
    max_turn: i32,
    hovers_in_place: bool,
}

impl FlyingMoveControl {
    /// Vanilla `FlyingMoveControl(mob, maxTurn, hoversInPlace)`.
    #[must_use]
    pub const fn new(max_turn: i32, hovers_in_place: bool) -> Self {
        Self {
            max_turn,
            hovers_in_place,
        }
    }

    /// Vanilla `FlyingMoveControl.tick`.
    #[must_use]
    pub fn tick(self, input: FlyingMoveControlInput) -> FlyingMoveControlOutput {
        let mut output = FlyingMoveControlOutput {
            y_rot: input.y_rot,
            x_rot: input.x_rot,
            speed: 0.0,
            yya: 0.0,
            zza: 0.0,
            no_gravity: false,
        };

        if input.operation != MoveControlOperation::MoveTo {
            // Vanilla: `if (!this.hoversInPlace) this.mob.setNoGravity(false);`
            // i.e. a hovering mob keeps the gravity flag it set while flying.
            output.no_gravity = self.hovers_in_place;
            output.yya = 0.0;
            output.zza = 0.0;
            return output;
        }

        output.no_gravity = true;

        let delta = input.wanted_position - input.position;
        if delta.length_squared() < 2.500_000_3e-7 {
            output.yya = 0.0;
            output.zza = 0.0;
            return output;
        }

        let y_rot_d = (delta.z.atan2(delta.x).to_degrees() as f32) - 90.0;
        output.y_rot = rotate_towards(input.y_rot, y_rot_d, 90.0);

        let attribute_speed = if input.on_ground {
            input.movement_speed
        } else {
            input.flying_speed
        };
        let speed = (input.speed_modifier * attribute_speed) as f32;
        output.speed = speed;
        // Vanilla reaches this through `mob.setSpeed(speed)`, and `Mob.setSpeed`
        // writes `setZza(speed)` as well as the cached speed: the MOVE_TO branch
        // must therefore leave the forward input at `speed`, not at zero.
        output.zza = speed;

        let horizontal = (delta.x * delta.x + delta.z * delta.z).sqrt();
        if delta.y.abs() > 1.0e-5 || horizontal.abs() > 1.0e-5 {
            let x_rot_d = -(delta.y.atan2(horizontal).to_degrees() as f32);
            output.x_rot = rotate_towards(input.x_rot, x_rot_d, self.max_turn as f32);
            output.yya = if delta.y > 0.0 { speed } else { -speed };
        }

        output
    }
}

/// Vanilla `FlyingMoveControl.tick` against a live mob.
///
/// See [`tick_smooth_swimming_move_control`] for why this lives here: vanilla
/// installs a `FlyingMoveControl` in the mob constructor and ticks it through
/// `MoveControl.tick`, while Steel's `Mob` trait lets a mob override
/// `tick_move_control`, which calls this with its own control instance.
pub(crate) fn tick_flying_move_control<M: Mob + ?Sized>(mob: &M, control: FlyingMoveControl) {
    let move_control = {
        let mut controls = mob.mob_base().controls().lock();
        let move_control = controls.move_control;
        if matches!(move_control.operation(), MoveControlOperation::MoveTo) {
            controls.move_control.set_wait();
        }
        move_control
    };

    let (y_rot, x_rot) = mob.rotation();
    let (movement_speed, flying_speed) = {
        let attributes = mob.attributes().lock();
        (
            attributes.required_value(vanilla_attributes::MOVEMENT_SPEED),
            attributes.required_value(vanilla_attributes::FLYING_SPEED),
        )
    };
    let output = control.tick(FlyingMoveControlInput {
        operation: move_control.operation(),
        wanted_position: move_control.wanted_position(),
        position: mob.position(),
        y_rot,
        x_rot,
        on_ground: mob.on_ground(),
        speed_modifier: move_control.speed_modifier(),
        movement_speed,
        flying_speed,
    });

    mob.set_rotation((output.y_rot, output.x_rot));
    // `Mob.setSpeed` is what vanilla calls here, so the forward input follows it;
    // the `setWantedPosition`-driven `MOVE_TO` step keeps it at the flying speed.
    mob.set_mob_speed(output.speed);
    mob.set_no_gravity(output.no_gravity);
    let input = mob.travel_input();
    mob.set_travel_input(LivingTravelInput::new(
        input.sideways(),
        output.yya,
        output.zza,
    ));
}

/// Vanilla `SmoothSwimmingMoveControl` input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothSwimmingMoveControlInput {
    /// Vanilla `MoveControl.Operation`.
    pub operation: MoveControlOperation,
    /// Vanilla `mob.getNavigation().isDone()`.
    pub navigation_done: bool,
    /// Vanilla `wantedX/Y/Z`.
    pub wanted_position: DVec3,
    /// Vanilla `mob.getX/Y/Z()`.
    pub position: DVec3,
    /// Vanilla `mob.getYRot()`.
    pub y_rot: f32,
    /// Vanilla `mob.getXRot()`.
    pub x_rot: f32,
    /// Vanilla `mob.isInWater()`.
    pub in_water: bool,
    /// Vanilla `speedModifier`.
    pub speed_modifier: f64,
    /// Vanilla `Attributes.MOVEMENT_SPEED`.
    pub movement_speed: f64,
}

/// Vanilla `SmoothSwimmingMoveControl` output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothSwimmingMoveControlOutput {
    /// Vanilla `mob.setYRot`.
    pub y_rot: f32,
    /// Vanilla `mob.setXRot`.
    pub x_rot: f32,
    /// Vanilla `mob.setSpeed`.
    pub speed: f32,
    /// Vanilla `mob.xxa`.
    pub xxa: f32,
    /// Vanilla `mob.yya`.
    pub yya: f32,
    /// Vanilla `mob.zza`.
    pub zza: f32,
    /// Vanilla `mob.yBodyRot` / `mob.yHeadRot` when the body follows the yaw.
    pub sync_body_and_head_rot: bool,
    /// Vanilla `mob.setDeltaMovement(delta + (0, 0.005, 0))` when `applyGravity`.
    pub apply_gravity_lift: bool,
}

/// Vanilla `SmoothSwimmingMoveControl<T extends Mob>`: turns the whole body toward
/// the path and swims along the pitch angle, with separate in/out-of-water speeds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothSwimmingMoveControl {
    max_turn_x: i32,
    max_turn_y: i32,
    in_water_speed_modifier: f32,
    outside_water_speed_modifier: f32,
    apply_gravity: bool,
}

impl SmoothSwimmingMoveControl {
    /// Vanilla `SmoothSwimmingMoveControl(mob, maxTurnX, maxTurnY,
    /// inWaterSpeedModifier, outsideWaterSpeedModifier, applyGravity)`.
    #[must_use]
    pub const fn new(
        max_turn_x: i32,
        max_turn_y: i32,
        in_water_speed_modifier: f32,
        outside_water_speed_modifier: f32,
        apply_gravity: bool,
    ) -> Self {
        Self {
            max_turn_x,
            max_turn_y,
            in_water_speed_modifier,
            outside_water_speed_modifier,
            apply_gravity,
        }
    }

    /// Vanilla `SmoothSwimmingMoveControl.getTurningSpeedFactor`.
    fn turning_speed_factor(left_to_turn: f32) -> f32 {
        1.0 - ((left_to_turn - 10.0) / 50.0).clamp(0.0, 1.0)
    }

    /// Vanilla `SmoothSwimmingMoveControl.tick`.
    #[must_use]
    pub fn tick(&self, input: SmoothSwimmingMoveControlInput) -> SmoothSwimmingMoveControlOutput {
        let mut output = SmoothSwimmingMoveControlOutput {
            y_rot: input.y_rot,
            x_rot: input.x_rot,
            speed: 0.0,
            xxa: 0.0,
            yya: 0.0,
            zza: 0.0,
            sync_body_and_head_rot: false,
            apply_gravity_lift: self.apply_gravity && input.in_water,
        };

        if input.operation != MoveControlOperation::MoveTo || input.navigation_done {
            output.speed = 0.0;
            output.xxa = 0.0;
            output.yya = 0.0;
            output.zza = 0.0;
            return output;
        }

        let delta = input.wanted_position - input.position;
        if delta.length_squared() < 2.500_000_3e-7 {
            // Vanilla returns before the trailing `applyGravity` lift.
            output.apply_gravity_lift = false;
            output.zza = 0.0;
            return output;
        }

        let y_rot_d = (delta.z.atan2(delta.x).to_degrees() as f32) - 90.0;
        output.y_rot = rotate_towards(input.y_rot, y_rot_d, self.max_turn_y as f32);
        output.sync_body_and_head_rot = true;

        let speed = (input.speed_modifier * input.movement_speed) as f32;
        if input.in_water {
            output.speed = speed * self.in_water_speed_modifier;

            let horizontal = (delta.x * delta.x + delta.z * delta.z).sqrt();
            if delta.y.abs() > 1.0e-5 || horizontal.abs() > 1.0e-5 {
                let mut x_rot_d = -(delta.y.atan2(horizontal).to_degrees() as f32);
                x_rot_d =
                    wrap_degrees(x_rot_d).clamp(-(self.max_turn_x as f32), self.max_turn_x as f32);
                output.x_rot = rotate_towards(input.x_rot, x_rot_d, 5.0);
            }

            let cos = (output.x_rot * PI / 180.0).cos();
            let sin = (output.x_rot * PI / 180.0).sin();
            output.zza = cos * speed;
            output.yya = -sin * speed;
        } else {
            let left_to_turn = wrap_degrees(input.y_rot - y_rot_d).abs();
            output.speed = speed
                * self.outside_water_speed_modifier
                * Self::turning_speed_factor(left_to_turn);
        }

        output
    }
}

/// Vanilla `SmoothSwimmingLookControl` input: the look state the control reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothSwimmingLookControlInput {
    /// Vanilla `LookControl.isLookingAtTarget`.
    pub looking_at_target: bool,
    /// Vanilla `wantedX/Y/Z`.
    pub wanted_position: DVec3,
    /// Vanilla `mob.getX/Y/Z()`.
    pub position: DVec3,
    /// Vanilla `mob.getEyeY()`.
    pub eye_y: f64,
    /// Vanilla `mob.yHeadRot`.
    pub y_head_rot: f32,
    /// Vanilla `mob.yBodyRot`.
    pub y_body_rot: f32,
    /// Vanilla `mob.getXRot()`.
    pub x_rot: f32,
    /// Vanilla `LookControl.yMaxRotSpeed`.
    pub y_max_rot_speed: f32,
    /// Vanilla `LookControl.xMaxRotAngle`.
    pub x_max_rot_angle: f32,
    /// Vanilla `mob.getNavigation().isDone()`.
    pub navigation_done: bool,
}

/// Vanilla `SmoothSwimmingLookControl` output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothSwimmingLookControlOutput {
    /// Vanilla `mob.yHeadRot`.
    pub y_head_rot: f32,
    /// Vanilla `mob.setXRot`.
    pub x_rot: f32,
    /// Vanilla `mob.yBodyRot`.
    pub y_body_rot: f32,
}

/// Vanilla `SmoothSwimmingLookControl<T extends Mob>`: keeps the head level while
/// swimming, offsets the pitch/yaw by the swim posture, and lets the body trail
/// the head by at most `max_y_rot_from_center` degrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmoothSwimmingLookControl {
    max_y_rot_from_center: i32,
}

impl SmoothSwimmingLookControl {
    /// Vanilla `SmoothSwimmingLookControl(mob, maxYRotFromCenter)`.
    #[must_use]
    pub const fn new(max_y_rot_from_center: i32) -> Self {
        Self {
            max_y_rot_from_center,
        }
    }

    /// Vanilla `SmoothSwimmingLookControl.tick`.
    #[must_use]
    pub fn tick(self, input: SmoothSwimmingLookControlInput) -> SmoothSwimmingLookControlOutput {
        let mut output = SmoothSwimmingLookControlOutput {
            y_head_rot: input.y_head_rot,
            x_rot: input.x_rot,
            y_body_rot: input.y_body_rot,
        };

        if input.looking_at_target {
            // Vanilla adds the swim posture offsets: +20 yaw, +10 pitch.
            if let Some(y_rot_d) = Self::y_rot_d(&input) {
                output.y_head_rot =
                    rotate_towards(input.y_head_rot, y_rot_d + 20.0, input.y_max_rot_speed);
            }
            if let Some(x_rot_d) = Self::x_rot_d(&input) {
                output.x_rot = rotate_towards(input.x_rot, x_rot_d + 10.0, input.x_max_rot_angle);
            }
        } else {
            if input.navigation_done {
                output.x_rot = rotate_towards(input.x_rot, 0.0, 5.0);
            }
            output.y_head_rot =
                rotate_towards(input.y_head_rot, input.y_body_rot, input.y_max_rot_speed);
        }

        let head_diff_body = wrap_degrees(output.y_head_rot - input.y_body_rot);
        if head_diff_body < -(self.max_y_rot_from_center as f32) {
            output.y_body_rot = input.y_body_rot - 4.0;
        } else if head_diff_body > self.max_y_rot_from_center as f32 {
            output.y_body_rot = input.y_body_rot + 4.0;
        }

        output
    }

    /// Vanilla `LookControl.getXRotD`.
    fn x_rot_d(input: &SmoothSwimmingLookControlInput) -> Option<f32> {
        let xd = input.wanted_position.x - input.position.x;
        let yd = input.wanted_position.y - input.eye_y;
        let zd = input.wanted_position.z - input.position.z;
        let horizontal = xd.hypot(zd);
        if yd.abs() > 1.0e-5 || horizontal.abs() > 1.0e-5 {
            Some(-(yd.atan2(horizontal).to_degrees() as f32))
        } else {
            None
        }
    }

    /// Vanilla `LookControl.getYRotD`.
    fn y_rot_d(input: &SmoothSwimmingLookControlInput) -> Option<f32> {
        let xd = input.wanted_position.x - input.position.x;
        let zd = input.wanted_position.z - input.position.z;
        if zd.abs() > 1.0e-5 || xd.abs() > 1.0e-5 {
            Some(zd.atan2(xd).to_degrees() as f32 - 90.0)
        } else {
            None
        }
    }
}

/// Vanilla `SmoothSwimmingLookControl.tick` against a live mob.
///
/// Lives next to the control type so every water mob shares one implementation:
/// vanilla installs `SmoothSwimmingLookControl` in the mob constructor and ticks
/// it through `LookControl.tick`; Steel's `Mob` trait lets a mob override
/// `tick_look_control`, which calls this.
pub(crate) fn tick_smooth_swimming_look_control<M: Mob + ?Sized>(
    mob: &M,
    control: SmoothSwimmingLookControl,
) {
    let look_control = {
        let mut controls = mob.mob_base().controls().lock();
        let look_control = controls.look_control;
        controls.look_control.tick_cooldown();
        look_control
    };

    let output = control.tick(SmoothSwimmingLookControlInput {
        looking_at_target: look_control.is_looking_at_target(),
        wanted_position: look_control.wanted_position(),
        position: mob.position(),
        eye_y: mob.get_eye_y(),
        y_head_rot: mob.y_head_rot(),
        y_body_rot: mob.y_body_rot(),
        x_rot: mob.rotation().1,
        y_max_rot_speed: look_control.y_max_rot_speed(),
        x_max_rot_angle: look_control.x_max_rot_angle(),
        navigation_done: mob.mob_base().navigation().lock().is_done(),
    });

    let (yaw, _) = mob.rotation();
    mob.set_rotation((yaw, output.x_rot));
    mob.set_y_head_rot(output.y_head_rot);
    mob.living_base().set_y_body_rot(output.y_body_rot);
}

/// Vanilla `SmoothSwimmingMoveControl.tick` against a live mob.
///
/// See [`tick_smooth_swimming_look_control`] for why this lives here.
pub(crate) fn tick_smooth_swimming_move_control<M: Mob + ?Sized>(
    mob: &M,
    control: SmoothSwimmingMoveControl,
) {
    let move_control = {
        let mut controls = mob.mob_base().controls().lock();
        let move_control = controls.move_control;
        if matches!(move_control.operation(), MoveControlOperation::MoveTo) {
            controls.move_control.set_wait();
        }
        move_control
    };

    let movement_speed = mob
        .attributes()
        .lock()
        .required_value(vanilla_attributes::MOVEMENT_SPEED);
    let output = control.tick(SmoothSwimmingMoveControlInput {
        operation: move_control.operation(),
        navigation_done: mob.mob_base().navigation().lock().is_done(),
        wanted_position: move_control.wanted_position(),
        position: mob.position(),
        y_rot: mob.rotation().0,
        x_rot: mob.rotation().1,
        in_water: mob.is_in_water(),
        speed_modifier: move_control.speed_modifier(),
        movement_speed,
    });

    mob.set_rotation((output.y_rot, output.x_rot));
    if output.sync_body_and_head_rot {
        mob.set_y_head_rot(output.y_rot);
        mob.living_base().set_y_body_rot(output.y_rot);
    }
    mob.set_speed(output.speed);
    let input = mob.travel_input();
    mob.set_travel_input(LivingTravelInput::new(
        input.sideways(),
        output.yya,
        output.zza,
    ));
    if output.apply_gravity_lift {
        mob.set_velocity(mob.velocity() + DVec3::new(0.0, 0.005, 0.0));
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::{
        BodyRotationControl, BodyRotationInput, FlyingMoveControl, FlyingMoveControlInput,
        MoveControlOperation,
    };

    fn assert_f32_close(left: f32, right: f32) {
        assert!(
            (left - right).abs() < 1.0e-6,
            "expected {left:?} to equal {right:?}"
        );
    }

    #[test]
    fn body_rotation_control_faces_body_forward_while_moving() {
        let mut control = BodyRotationControl::new();

        let update = control.tick(BodyRotationInput::new(true, false, 90.0, 0.0, 200.0, 75.0));

        assert_f32_close(update.y_body_rot(), 90.0);
        assert_f32_close(update.y_head_rot(), 165.0);
        assert_eq!(control.head_stable_time(), 0);
        assert_f32_close(control.last_stable_y_head_rot(), 165.0);
    }

    #[test]
    fn body_rotation_control_turns_body_when_idle_head_moves() {
        let mut control = BodyRotationControl::new();

        let update = control.tick(BodyRotationInput::new(false, false, 0.0, 0.0, 90.0, 75.0));

        assert_f32_close(update.y_body_rot(), 15.0);
        assert_f32_close(update.y_head_rot(), 90.0);
        assert_eq!(control.head_stable_time(), 0);
        assert_f32_close(control.last_stable_y_head_rot(), 90.0);
    }

    #[test]
    fn body_rotation_control_waits_then_turns_body_toward_stable_head() {
        let mut control = BodyRotationControl::new();
        let first = control.tick(BodyRotationInput::new(false, false, 0.0, 0.0, 90.0, 75.0));
        let mut y_body_rot = first.y_body_rot();

        for _ in 0..10 {
            y_body_rot = control
                .tick(BodyRotationInput::new(
                    false, false, 0.0, y_body_rot, 90.0, 75.0,
                ))
                .y_body_rot();
        }

        assert_eq!(control.head_stable_time(), 10);
        assert_f32_close(y_body_rot, 15.0);

        let update = control.tick(BodyRotationInput::new(
            false, false, 0.0, y_body_rot, 90.0, 75.0,
        ));

        assert_eq!(control.head_stable_time(), 11);
        assert_f32_close(update.y_body_rot(), 22.5);
    }

    #[test]
    fn body_rotation_control_does_not_turn_idle_body_when_carrying_mob_passenger() {
        let mut control = BodyRotationControl::new();

        let update = control.tick(BodyRotationInput::new(false, true, 0.0, 0.0, 90.0, 75.0));

        assert_f32_close(update.y_body_rot(), 0.0);
        assert_f32_close(update.y_head_rot(), 90.0);
        assert_eq!(control.head_stable_time(), 0);
        assert_f32_close(control.last_stable_y_head_rot(), 0.0);
    }

    #[test]
    fn flying_move_to_keeps_the_forward_input_at_the_flying_speed() {
        let output = FlyingMoveControl::new(10, false).tick(FlyingMoveControlInput {
            operation: MoveControlOperation::MoveTo,
            wanted_position: DVec3::new(10.0, 74.0, 0.5),
            position: DVec3::new(0.5, 70.0, 0.5),
            y_rot: 0.0,
            x_rot: 0.0,
            on_ground: false,
            speed_modifier: 1.0,
            movement_speed: 0.1,
            flying_speed: 0.4,
        });

        // Vanilla reaches `setZza(speed)` through `Mob.setSpeed`; without it a caller
        // that maps `zza` onto the forward travel input leaves the mob spinning in place.
        assert_f32_close(output.speed, 0.4);
        assert_f32_close(output.zza, output.speed);
        assert_f32_close(output.yya, output.speed);
    }

    #[test]
    fn flying_move_wait_zeroes_the_forward_input() {
        let output = FlyingMoveControl::new(10, false).tick(FlyingMoveControlInput {
            operation: MoveControlOperation::Wait,
            wanted_position: DVec3::new(10.0, 74.0, 0.5),
            position: DVec3::new(0.5, 70.0, 0.5),
            y_rot: 0.0,
            x_rot: 0.0,
            on_ground: false,
            speed_modifier: 1.0,
            movement_speed: 0.1,
            flying_speed: 0.4,
        });

        assert_f32_close(output.zza, 0.0);
        assert_f32_close(output.yya, 0.0);
        assert!(
            !output.no_gravity,
            "a non-hovering flyer drops out of hover"
        );
    }

    /// The real flying path: a waypoint drives the navigation, which drives
    /// [`tick_flying_move_control`], which drives `LivingEntity::travel` and the
    /// world's `move()` inside an entity-ticking chunk.
    mod live_flight {
        use std::sync::Arc;

        use glam::DVec3;
        use steel_registry::vanilla_game_rules::{SPAWN_MOBS, SPAWN_MONSTERS};
        use steel_registry::{init_vanilla_registry, vanilla_entities};
        use steel_utils::ChunkPos;

        use crate::behavior::init_behaviors;
        use crate::entity::entities::mobs::{AllayEntity, BeeEntity, ParrotEntity};
        use crate::entity::{
            Entity, LivingEntity, PathfinderMob, SharedEntity, add_spawned_entity, init_entities,
        };
        use crate::player::ResetReason;
        use crate::test_support::{
            TestPlayerBuilder, TestWorld, fresh_test_world, insert_entity_ticking_chunks,
            settle_entity_ticking_chunk,
        };
        use crate::world::World;

        /// Upper bound on the world ticks a test may burn waiting for entity ticks.
        const MAX_WORLD_TICKS: u64 = 4_000;

        /// Radius, in chunks, of the entity-ticking region a player holds in a test
        /// world: the player's loading/simulation tickets cover a square of
        /// `world.simulation_distance` (2), and the readiness derivation needs that
        /// square's own 5x5 `Full` neighborhood, hence a 9x9 region.
        const PLAYER_TICKING_RADIUS: i32 = 4;

        /// A fresh test world whose origin chunk is entity-ticking, held open by a
        /// watcher player exactly like a running server does.
        fn ticking_world(key: &'static str) -> TestWorld {
            init_vanilla_registry();
            init_entities();
            init_behaviors();

            let fixture = fresh_test_world(key);
            let world = &fixture.world;
            let center = ChunkPos::new(0, 0);
            // The player's join queues its tickets, and the next chunk source pass
            // re-derives every chunk's readiness from its neighborhood; loading the
            // 9x9 `Full` region first is what keeps the origin chunk entity-ticking
            // through that pass instead of demoting it and starving the mob.
            insert_entity_ticking_chunks(world, center, PLAYER_TICKING_RADIUS);

            // A player keeps the chunk entity-ticking so `tick_game` steps the mob.
            let watcher = TestPlayerBuilder::new(Arc::clone(world), "FlyingWatcher", 1).build();
            watcher.set_client_loaded(true);
            watcher.snap_to(DVec3::new(8.5, 70.0, 8.5), 0.0, 0.0);
            assert!(
                world.add_player(Arc::clone(&watcher), ResetReason::InitialJoin),
                "the watcher must hold the chunk active"
            );
            settle_entity_ticking_chunk(world, center);

            // Vanilla's natural-spawn pass is unrelated to flying movement and depends
            // on the process-wide RNG.
            world.set_game_rule(&SPAWN_MOBS, false);
            world.set_game_rule(&SPAWN_MONSTERS, false);
            fixture
        }

        /// Steps the world until the mob has been ticked `entity_ticks` more times.
        ///
        /// Entity ticking only starts once the watcher player's chunk tickets have
        /// propagated to the chunk holder (a few dozen world ticks in), and one world
        /// tick steps an entity at most once, so the test budget is counted in the
        /// mob's own tick counter instead of raw world ticks.
        fn tick_entity_ticks(
            world: &Arc<World>,
            tick: &mut u64,
            mob: &dyn Entity,
            entity_ticks: i32,
            mut observe: impl FnMut(),
        ) {
            let target = mob.tick_count() + entity_ticks;
            while mob.tick_count() < target && *tick < MAX_WORLD_TICKS {
                world.game_time.advance();
                world.tick_game(*tick, true);
                observe();
                *tick += 1;
            }
        }

        /// Waits for the entity-ticking chunk to actually start stepping the mob.
        fn tick_until_first_entity_tick(world: &Arc<World>, tick: &mut u64, mob: &dyn Entity) {
            tick_entity_ticks(world, tick, mob, 1, || {});
            assert!(
                mob.tick_count() > 0,
                "the entity-ticking chunk must step the mob"
            );
        }

        /// Tracks how far the mob got from where it started.
        #[derive(Debug, Clone, Copy)]
        struct Progress {
            start: DVec3,
            max_horizontal: f64,
            max_y_gain: f64,
        }

        impl Progress {
            fn observe(&mut self, position: DVec3) {
                self.max_horizontal = self
                    .max_horizontal
                    .max((position.x - self.start.x).hypot(position.z - self.start.z));
                self.max_y_gain = self.max_y_gain.max(position.y - self.start.y);
            }
        }

        fn progress_from(start: DVec3) -> Progress {
            Progress {
                start,
                max_horizontal: 0.0,
                max_y_gain: 0.0,
            }
        }

        /// A flying mob with a raised waypoint must move along both axes: the shipped
        /// bug left the forward travel input at zero, so the mob only yawed/pitched and
        /// sank along its own pitch.
        #[test]
        #[expect(
            clippy::clone_on_ref_ptr,
            reason = "the plain clone unsizes to `Arc<dyn Entity>`; `Arc::clone` cannot take the already-coerced reference in one expression"
        )]
        fn a_parrot_with_a_waypoint_flies_there() {
            let fixture = ticking_world("flying_parrot_waypoint");
            let world = &fixture.world;

            let parrot = Arc::new(ParrotEntity::new(
                &vanilla_entities::PARROT,
                2,
                DVec3::new(0.5, 70.0, 0.5),
                Arc::downgrade(world),
            ));
            // The parrot's wander goals would overwrite the waypoint under test.
            PathfinderMob::remove_all_goals(parrot.as_ref(), |_| true);

            let shared: SharedEntity = parrot.clone();
            add_spawned_entity(world, shared).expect("the parrot joins the loaded chunk");

            assert!(
                parrot.move_to_pos(DVec3::new(10.5, 74.0, 2.5), 1.0),
                "the flying navigation must accept a waypoint path"
            );

            let mut tick = 0_u64;
            tick_until_first_entity_tick(world, &mut tick, parrot.as_ref());

            let mut progress = progress_from(parrot.position());
            tick_entity_ticks(world, &mut tick, parrot.as_ref(), 120, || {
                progress.observe(parrot.position());
            });

            assert!(
                progress.max_horizontal > 3.0,
                "the parrot must fly toward the waypoint instead of turning in place ({progress:?})"
            );
            assert!(
                progress.max_y_gain > 2.0,
                "the parrot must climb to the raised waypoint ({progress:?})"
            );
        }

        /// A bee must fly too; it also has to hold the altitude the flying control's
        /// `hoversInPlace` flag gives it instead of dropping out of the air.
        #[test]
        #[expect(
            clippy::clone_on_ref_ptr,
            reason = "the plain clone unsizes to `Arc<dyn Entity>`; `Arc::clone` cannot take the already-coerced reference in one expression"
        )]
        fn a_bee_with_a_waypoint_flies_there_and_hovers() {
            let fixture = ticking_world("flying_bee_waypoint");
            let world = &fixture.world;

            let bee = Arc::new(BeeEntity::new(
                &vanilla_entities::BEE,
                2,
                DVec3::new(0.5, 70.0, 0.5),
                Arc::downgrade(world),
            ));
            PathfinderMob::remove_all_goals(bee.as_ref(), |_| true);

            let shared: SharedEntity = bee.clone();
            add_spawned_entity(world, shared).expect("the bee joins the loaded chunk");

            assert!(
                bee.move_to_pos(DVec3::new(10.5, 74.0, 2.5), 1.0),
                "the flying navigation must accept a waypoint path"
            );

            let mut tick = 0_u64;
            tick_until_first_entity_tick(world, &mut tick, bee.as_ref());

            let mut progress = progress_from(bee.position());
            tick_entity_ticks(world, &mut tick, bee.as_ref(), 200, || {
                progress.observe(bee.position());
            });

            assert!(
                progress.max_horizontal > 3.0,
                "the bee must fly toward the waypoint instead of hopping on the ground ({progress:?})"
            );
            assert!(
                progress.max_y_gain > 2.0,
                "the bee must climb to the raised waypoint ({progress:?})"
            );
            assert!(
                bee.position().y - progress.start.y > 2.0,
                "the bee must hover at the altitude it flew to, not sink ({progress:?})"
            );
        }

        /// An allay's `travel` is `travelFlying`, which never subtracts gravity; that is
        /// what keeps an allay hovering instead of dropping out of the air.
        ///
        /// An idle allay's own walk targets are random strolls, which legitimately fly
        /// it up and down, so the gravity-free part is asserted on one real travel step
        /// with no movement input rather than on wherever the stroll took the allay.
        #[test]
        #[expect(
            clippy::clone_on_ref_ptr,
            reason = "the plain clone unsizes to `Arc<dyn Entity>`; `Arc::clone` cannot take the already-coerced reference in one expression"
        )]
        fn an_allay_hover_step_does_not_apply_gravity() {
            let fixture = ticking_world("flying_allay_hover");
            let world = &fixture.world;

            let allay = Arc::new(AllayEntity::new(
                &vanilla_entities::ALLAY,
                2,
                DVec3::new(0.5, 100.0, 0.5),
                Arc::downgrade(world),
            ));
            let shared: SharedEntity = allay.clone();
            add_spawned_entity(world, Arc::clone(&shared))
                .expect("the allay joins the loaded chunk");

            // The allay has to sit in an entity-ticking chunk, so that the step below
            // runs against its live world.
            let mut tick = 0_u64;
            tick_until_first_entity_tick(world, &mut tick, allay.as_ref());

            let start = allay.position();
            let velocity_y = allay.velocity().y;
            // Vanilla `LivingEntity.aiStep` runs this every tick with the allay's
            // movement input, which is zero while it has nowhere to fly.
            assert!(
                allay.travel(&shared, DVec3::ZERO).is_some(),
                "the allay's travel must move it inside the world"
            );

            assert!(
                allay.velocity().y >= velocity_y - 1.0e-9,
                "an allay must hover: travel must not add gravity (velocity.y {velocity_y} -> {})",
                allay.velocity().y
            );
            assert!(
                allay.position().y >= start.y - 1.0e-9,
                "an allay must hover instead of sinking ({} -> {})",
                start.y,
                allay.position().y
            );
        }
    }
}
