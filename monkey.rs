use crate::platform::{InputEvent, Rect};
use log::{info, warn};
use rand::Rng;
use std::time::{Duration, Instant};

// Constants from project plan and reasonable defaults
const HUNTING_INACTIVITY_SECONDS: u64 = 300;
const HUNTING_SPEED_PX_PER_FRAME: f32 = 3.0;
const HUNTING_DISTANCE_THRESHOLD_PX: f32 = 10.0;

const GROOMING_VELOCITY_THRESHOLD: f32 = 0.05;
const GROOMING_FRAME_THRESHOLD: u32 = 120;

const SCRATCHING_DECAY_MS: u64 = 750;

const WIGGLE_FRAME_INTERVAL: u32 = 500;
const WIGGLE_DURATION_MS: u64 = 400;

const DRAMATIC_ACCELERATION_THRESHOLD: f32 = 2500.0; // In pixels/sec^2, requires tuning
const GRAVITY_PX_PER_FRAME_SQ: f32 = 9.8;

const STRETCH_FACTOR: f32 = 0.05; // k_stretch
const SQUISH_FACTOR: f32 = 0.025; // k_squish = 0.5 * k_stretch

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum PetState {
    Idle,
    Dragged,
    Hunting,
    Grooming,
    Scratching,
    Dramatic,
    Wiggling, // Transient state for micro-wiggles
}

pub struct Monkey {
    pub state: PetState,
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    pub scale: (f32, f32),
    pub sprite_width: u32,
    pub sprite_height: u32,
    pub animation_frame: u32,
    pub eye_direction_frame: u32, // For eye-following

    // State-specific data
    last_mouse_pos: (f32, f32),
    last_mouse_velocity: (f32, f32),
    cursor_inactive_timer: Instant,
    grooming_timer: u32,
    scratch_decay_timer: Option<Instant>,
    wiggle_timer: u32,
    wiggle_end_timer: Option<Instant>,
    drag_anchor_offset: (f32, f32),
    hunting_speed: f32,
}

impl Monkey {
    pub fn new(start_pos: (f32, f32), sprite_width: u32, sprite_height: u32) -> Self {
        Self {
            state: PetState::Idle,
            position: start_pos,
            velocity: (0.0, 0.0),
            scale: (1.0, 1.0),
            sprite_width,
            sprite_height,
            animation_frame: 0,
            eye_direction_frame: 0,
            last_mouse_pos: (0.0, 0.0),
            last_mouse_velocity: (0.0, 0.0),
            cursor_inactive_timer: Instant::now(),
            grooming_timer: 0,
            scratch_decay_timer: None,
            wiggle_timer: 0,
            wiggle_end_timer: None,
            drag_anchor_offset: (0.0, 0.0),
            hunting_speed: HUNTING_SPEED_PX_PER_FRAME,
        }
    }

    /// Main update function for the monkey's state machine.
    pub fn tick(&mut self, input_events: &[InputEvent], dt: f32, screen_bounds: &[Rect]) {
        let mut current_mouse_pos = self.last_mouse_pos;
        let mut mouse_moved = false;
        let mut mouse_down = false;
        let mut mouse_up = false;

        // --- Input Processing ---
        for event in input_events {
            match event {
                InputEvent::MouseMove { x, y } => {
                    current_mouse_pos = (*x, *y);
                    mouse_moved = true;
                }
                InputEvent::MouseDown { x, y, button: _ } => {
                    current_mouse_pos = (*x, *y);
                    mouse_down = true;
                }
                InputEvent::MouseUp { x, y, button: _ } => {
                    current_mouse_pos = (*x, *y);
                    mouse_up = true;
                }
                InputEvent::KeyDown { key_code: _ } => {
                    if self.state != PetState::Dragged {
                        self.state = PetState::Scratching;
                        self.scratch_decay_timer =
                            Some(Instant::now() + Duration::from_millis(SCRATCHING_DECAY_MS));
                        info!("Monkey: Transitioned to Scratching state due to key press.");
                    }
                }
                _ => {}
            }
        }

        let mouse_velocity_x = (current_mouse_pos.0 - self.last_mouse_pos.0) / dt;
        let mouse_velocity_y = (current_mouse_pos.1 - self.last_mouse_pos.1) / dt;
        let mouse_velocity_magnitude = (mouse_velocity_x.powi(2) + mouse_velocity_y.powi(2)).sqrt();
        let mouse_acceleration_magnitude = ((mouse_velocity_x - self.last_mouse_velocity.0).powi(2)
            + (mouse_velocity_y - self.last_mouse_velocity.1).powi(2))
        .sqrt()
            / dt;

        if mouse_moved {
            self.cursor_inactive_timer = Instant::now();
        }
        self.last_mouse_pos = current_mouse_pos;
        self.last_mouse_velocity = (mouse_velocity_x, mouse_velocity_y);

        let monkey_bbox = self.get_bounding_box();
        let mouse_in_box = current_mouse_pos.0 >= monkey_bbox.x as f32
            && current_mouse_pos.0 <= (monkey_bbox.x + monkey_bbox.width as i32) as f32
            && current_mouse_pos.1 >= monkey_bbox.y as f32
            && current_mouse_pos.1 <= (monkey_bbox.y + monkey_bbox.height as i32) as f32;

        // --- State Machine Logic ---
        match self.state {
            PetState::Idle => {
                self.update_eye_direction(current_mouse_pos);

                if mouse_down && mouse_in_box {
                    self.state = PetState::Dragged;
                    self.drag_anchor_offset = (
                        current_mouse_pos.0 - self.position.0,
                        current_mouse_pos.1 - self.position.1,
                    );
                    info!("Monkey: Transitioned to Dragged state.");
                } else if self.cursor_inactive_timer.elapsed().as_secs() >= HUNTING_INACTIVITY_SECONDS {
                    self.state = PetState::Hunting;
                    info!("Monkey: Transitioned to Hunting state due to cursor inactivity.");
                } else if mouse_in_box && mouse_velocity_magnitude < GROOMING_VELOCITY_THRESHOLD {
                    self.grooming_timer += 1;
                    if self.grooming_timer >= GROOMING_FRAME_THRESHOLD {
                        self.state = PetState::Grooming;
                        info!("Monkey: Transitioned to Grooming state.");
                    }
                } else {
                    self.grooming_timer = 0;
                }

                self.wiggle_timer += 1;
                if self.wiggle_timer >= WIGGLE_FRAME_INTERVAL {
                    self.wiggle_timer = 0;
                    if rand::thread_rng().gen_bool(0.3) {
                        self.state = PetState::Wiggling;
                        self.wiggle_end_timer =
                            Some(Instant::now() + Duration::from_millis(WIGGLE_DURATION_MS));
                        info!("Monkey: Transitioned to Wiggling state.");
                    }
                }
            }
            PetState::Dragged => {
                if mouse_acceleration_magnitude > DRAMATIC_ACCELERATION_THRESHOLD {
                    self.state = PetState::Dramatic;
                    self.velocity = (self.last_mouse_velocity.0 * 0.5, self.last_mouse_velocity.1 * 0.5);
                    info!("Monkey: Transitioned to Dramatic state due to high acceleration.");
                } else if mouse_up {
                    self.state = PetState::Idle;
                    self.scale = (1.0, 1.0);
                    info!("Monkey: Transitioned to Idle state from Dragged.");
                } else {
                    self.position = (
                        current_mouse_pos.0 - self.drag_anchor_offset.0,
                        current_mouse_pos.1 - self.drag_anchor_offset.1,
                    );
                    let dy = current_mouse_pos.1 - self.last_mouse_pos.1;
                    self.scale.1 = (1.0 + (dy * STRETCH_FACTOR)).max(0.5).min(1.5);
                    self.scale.0 = (1.0 - (dy * SQUISH_FACTOR)).max(0.5).min(1.5);
                }
            }
            PetState::Hunting => {
                if mouse_moved {
                    self.state = PetState::Idle;
                    info!("Monkey: Transitioned to Idle state from Hunting (mouse moved).");
                } else {
                    let (target_x, target_y) = current_mouse_pos;
                    let monkey_cx = self.position.0 + (self.sprite_width as f32 * self.scale.0) / 2.0;
                    let monkey_cy = self.position.1 + (self.sprite_height as f32 * self.scale.1) / 2.0;
                    let dx = target_x - monkey_cx;
                    let dy = target_y - monkey_cy;
                    let distance = (dx.powi(2) + dy.powi(2)).sqrt();

                    if distance < self.hunting_speed || distance < HUNTING_DISTANCE_THRESHOLD_PX {
                        self.state = PetState::Idle;
                        self.position.0 = target_x - (self.sprite_width as f32 * self.scale.0) / 2.0;
                        self.position.1 = target_y - (self.sprite_height as f32 * self.scale.1) / 2.0;
                        info!("Monkey: Transitioned to Idle state from Hunting (reached target).");
                    } else {
                        let normalized_dx = dx / distance;
                        let normalized_dy = dy / distance;
                        self.position.0 += normalized_dx * self.hunting_speed;
                        self.position.1 += normalized_dy * self.hunting_speed;
                    }
                }
            }
            PetState::Grooming => {
                self.update_eye_direction(current_mouse_pos);
                let time = self.grooming_timer as f32 / 60.0;
                let breath = 1.0 + 0.02 * (time * std::f32::consts::PI * 2.0).sin();
                self.scale = (breath, breath);
                self.grooming_timer += 1;

                if !mouse_in_box || mouse_velocity_magnitude > GROOMING_VELOCITY_THRESHOLD {
                    self.state = PetState::Idle;
                    self.grooming_timer = 0;
                    self.scale = (1.0, 1.0);
                    info!("Monkey: Transitioned to Idle state from Grooming.");
                }
            }
            PetState::Scratching => {
                if let Some(end_time) = self.scratch_decay_timer {
                    if Instant::now() > end_time {
                        self.state = PetState::Idle;
                        self.scratch_decay_timer = None;
                        info!("Monkey: Transitioned to Idle state from Scratching.");
                    }
                }
            }
            PetState::Dramatic => {
                self.velocity.1 += GRAVITY_PX_PER_FRAME_SQ * dt;
                self.position.0 += self.velocity.0 * dt;
                self.position.1 += self.velocity.1 * dt;

                let monkey_bottom = self.position.1 + self.sprite_height as f32 * self.scale.1;
                if let Some(bounds) = screen_bounds.iter().find(|b| {
                    let floor_y = (b.y + b.height as i32) as f32;
                    monkey_bottom >= floor_y
                }) {
                    self.position.1 = (bounds.y + bounds.height as i32) as f32 - self.sprite_height as f32 * self.scale.1;
                    self.state = PetState::Idle;
                    self.velocity = (0.0, 0.0);
                    info!("Monkey: Transitioned to Idle state from Dramatic (hit floor).");
                }
            }
            PetState::Wiggling => {
                if let Some(end_time) = self.wiggle_end_timer {
                    if Instant::now() > end_time {
                        self.state = PetState::Idle;
                        self.wiggle_end_timer = None;
                        info!("Monkey: Transitioned to Idle state from Wiggling.");
                    } else {
                        let wiggle_x = (rand::thread_rng().gen::<f32>() - 0.5) * 4.0;
                        self.position.0 += wiggle_x;
                    }
                }
            }
        }

        if self.state != PetState::Dramatic {
            self.clamp_to_screen(screen_bounds);
        }
    }

    fn update_eye_direction(&mut self, mouse_pos: (f32, f32)) {
        let monkey_center_x = self.position.0 + (self.sprite_width as f32 * self.scale.0) / 2.0;
        let monkey_center_y = self.position.1 + (self.sprite_height as f32 * self.scale.1) / 2.0;

        let angle = (mouse_pos.1 - monkey_center_y).atan2(mouse_pos.0 - monkey_center_x);
        let angle_deg = angle.to_degrees() + 180.0;

        let sector = ((angle_deg + 22.5) / 45.0) % 8.0;
        self.eye_direction_frame = sector.floor() as u32;
    }

    fn get_bounding_box(&self) -> Rect {
        Rect {
            x: self.position.0 as i32,
            y: self.position.1 as i32,
            width: (self.sprite_width as f32 * self.scale.0) as u32,
            height: (self.sprite_height as f32 * self.scale.1) as u32,
        }
    }

    fn clamp_to_screen(&mut self, screen_bounds: &[Rect]) {
        if screen_bounds.is_empty() {
            return;
        }

        let monkey_bbox = self.get_bounding_box();
        let mut total_bounds_x_min = i32::MAX;
        let mut total_bounds_x_max = i32::MIN;
        let mut total_bounds_y_min = i32::MAX;
        let mut total_bounds_y_max = i32::MIN;

        for bounds in screen_bounds {
            total_bounds_x_min = total_bounds_x_min.min(bounds.x);
            total_bounds_x_max = total_bounds_x_max.max(bounds.x + bounds.width as i32);
            total_bounds_y_min = total_bounds_y_min.min(bounds.y);
            total_bounds_y_max = total_bounds_y_max.max(bounds.y + bounds.height as i32);
        }

        self.position.0 = self.position.0.max(total_bounds_x_min as f32);
        self.position.0 = self.position.0.min((total_bounds_x_max - monkey_bbox.width as i32) as f32);
        self.position.1 = self.position.1.max(total_bounds_y_min as f32);
        self.position.1 = self.position.1.min((total_bounds_y_max - monkey_bbox.height as i32) as f32);
    }
}