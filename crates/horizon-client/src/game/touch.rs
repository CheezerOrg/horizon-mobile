//! Touch controls on the window (the iOS port; the gestures are in
//! `horizon_client_input::touch`). While playing, the stick moves the player
//! as a pad's left stick does, the view finger turns it as mouse motion,
//! and the buttons act as the vanilla key mappings they stand for. Over
//! menus the fingers make the mouse's window events. On by default on iOS
//! only; `HORIZON_TOUCH=1` turns it on elsewhere (a touchscreen). The Touch
//! Controls page sets their size, opacity, look speed and side
//! (`TouchSettings`, in `settings.controller.touch`). They are drawn with
//! Bedrock Edition's own textures when the resources have them
//! (`textures/gui/bedrock`, which `ios/bedrock_controls.sh` downloads: Mojang's
//! files, never in the repository), and as plain shapes otherwise. While the
//! resources still load, the startup menu's title takes taps the same way
//! (`Loading`), before `Game` exists.
use super::event_loop::EventLoopCtl;
use super::input_trace::InputOrigin;
use super::{Game, Loading, Screen};
use horizon_client::held::Source;
use horizon_client::{
    Result,
    hud::{self, HudAtlas, HudBatch, rgba},
    keybinds::{KeyBinds, key_code, mouse_button},
    keymapping::SNEAK,
};
use horizon_client_input::touch::{
    Button, Control, JUMP_SIDE, Layout, Mode, Output, Phase, Rect, TouchInput, available as enabled,
};
use std::time::Instant;
use winit::{
    dpi::PhysicalPosition,
    event::{
        DeviceId, ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent,
    },
    keyboard::{KeyCode, ModifiersState},
};

/// Kept clear at the sides of a phone with a notch, in points.
const NOTCH_POINTS: f32 = 48.0;
/// The view turns as far as a mouse moved this many counts per point of
/// finger travel (the mouse sensitivity applies on top).
const LOOK_COUNTS_PER_POINT: f32 = 2.5;
const HOTBAR_KEYS: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

#[derive(Debug, Default)]
pub(super) struct Touches {
    input: TouchInput,
}

impl Game<'_> {
    /// Where the controls sit, and the GUI scale (window pixels per GUI pixel).
    fn touch_layout(&self) -> Option<(Layout, f32)> {
        let batch = self.gui_scale()?;
        let points = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let per_point = points / batch.scale;
        // Phones with a notch are wider than 16:9; tablets and older phones have no notch.
        let notched = batch.width / batch.height > 1.9;
        let settings = &self.settings.controller.touch;
        let player = self.player.as_ref();
        let layout = Layout {
            width: batch.width,
            height: batch.height,
            unit: JUMP_SIDE * batch.height * settings.scale(),
            inset: if notched {
                NOTCH_POINTS * per_point
            } else {
                0.0
            },
            left_handed: settings.left_handed,
            minimal: !settings.enabled,
            flying: player.is_some_and(|p| p.flying || p.fluids.was_touching_water),
        };
        Some((layout, batch.scale))
    }

    fn touch_mode(&self) -> Mode {
        if self.pad_in_menu() || self.player.is_none() {
            Mode::Menu {
                back: self.title.is_none(),
            }
        } else {
            Mode::Play
        }
    }

    /// `WindowEvent::Touch`; true when the touch controls took it.
    pub(super) fn touch_event(&mut self, event_loop: &dyn EventLoopCtl, touch: &Touch) -> bool {
        if !enabled() {
            return false;
        }
        let Some((layout, scale)) = self.touch_layout() else {
            return true;
        };
        let phase = phase(touch.phase);
        let pos = [
            touch.location.x as f32 / scale,
            touch.location.y as f32 / scale,
        ];
        self.last_input = Instant::now();
        let mode = self.touch_mode();
        self.touch.input.set_mode(mode);
        self.touch
            .input
            .event(touch.id, phase, pos, Instant::now(), &layout);
        self.touch_apply(event_loop, scale);
        // A finger on a screen with a focused text box brings back the
        // keyboard the player hid (iOS hides it without telling the game).
        // Only once it lifts, after its click: the keyboard shrinks the view
        // and moves the screen's widgets, so showing it under a finger still
        // down sends that finger's click to where the widgets were.
        if phase == Phase::Ended && mode != Mode::Play {
            // The click may have moved the focus or closed the screen.
            self.sync_ime();
            if self.ime_allowed
                && let Some(window) = &self.window
            {
                window.set_ime_allowed(true);
            }
        }
        true
    }

    /// `horizon-ctl touch start|move|end|cancel ID X Y`: a finger at GUI
    /// pixel X, Y, as the window would report it.
    pub(super) fn touch_command(
        &mut self,
        words: &[&str],
        event_loop: &dyn EventLoopCtl,
    ) -> Result<serde_json::Value> {
        const USAGE: &str = "touch start|move|end|cancel ID X Y";
        let [_, phase, id, x, y] = words else {
            return Err(USAGE.into());
        };
        let phase = match *phase {
            "start" => TouchPhase::Started,
            "move" => TouchPhase::Moved,
            "end" => TouchPhase::Ended,
            "cancel" => TouchPhase::Cancelled,
            _ => return Err(USAGE.into()),
        };
        let number = |s: &str| s.parse::<f64>().map_err(|_| USAGE.to_string());
        let id = id.parse::<u64>().map_err(|_| USAGE.to_string())?;
        let scale = f64::from(self.touch_layout().ok_or("no window yet")?.1);
        let touch = Touch {
            device_id: DeviceId::dummy(),
            phase,
            location: PhysicalPosition::new(number(x)? * scale, number(y)? * scale),
            force: None,
            id,
        };
        if !self.touch_event(event_loop, &touch) {
            return Err("touch controls are off (HORIZON_TOUCH=1 turns them on)".into());
        }
        Ok(serde_json::json!({ "mode": format!("{:?}", self.touch_mode()) }))
    }

    /// Once per loop iteration: long presses, held attacks, sprinting.
    pub(super) fn touch_frame(&mut self, event_loop: &dyn EventLoopCtl) {
        if !enabled() {
            return;
        }
        let Some((_, scale)) = self.touch_layout() else {
            return;
        };
        self.touch.input.set_mode(self.touch_mode());
        self.touch.input.frame(Instant::now());
        self.touch_apply(event_loop, scale);
    }

    /// The touch stick's move vector while playing, as `pad_move` gives a pad's.
    pub(super) fn touch_move(&self) -> Option<[f32; 2]> {
        if !enabled() || self.pad_in_menu() {
            return None;
        }
        self.touch.input.movement()
    }

    /// The touch controls hold sneak (slot 0: the descend button) or sprint
    /// (slot 1: its button, or the stick pushed forward) with no Shift or Ctrl
    /// behind it, so `heal_hold_latches` must leave the latch alone.
    pub(super) fn touch_holds_latch(&self, slot: usize) -> bool {
        enabled()
            && match slot {
                0 => self.touch.input.holding(Control::Descend),
                1 => self.touch.input.sprinting(),
                _ => false,
            }
    }

    fn touch_apply(&mut self, event_loop: &dyn EventLoopCtl, scale: f32) {
        let [dx, dy] = self.touch.input.take_look();
        if [dx, dy] != [0.0, 0.0] {
            // GUI pixels to points, then to mouse counts.
            let points = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
            let k = scale / points
                * LOOK_COUNTS_PER_POINT
                * self.settings.controller.touch.look_scale();
            self.mouse.add_motion(f64::from(dx * k), f64::from(dy * k));
        }
        for out in self.touch.input.take() {
            if let Err(e) = self.touch_output(event_loop, out, scale) {
                log::warn!("Horizon: touch {out:?}: {e}");
            }
        }
    }

    fn touch_output(
        &mut self,
        event_loop: &dyn EventLoopCtl,
        out: Output,
        scale: f32,
    ) -> Result<()> {
        let playing = !self.pad_in_menu();
        match out {
            Output::Press(Control::Pause | Control::Back) => self.pad_back(),
            Output::Press(Control::Sneak) if playing => {
                if self.settings.toggle_crouch {
                    // Vanilla's own toggle flips on the press.
                    self.mapped_key(
                        KeyCode::ShiftLeft,
                        true,
                        false,
                        KeyCode::ShiftLeft,
                        Source::Pad,
                    )?;
                    self.mapped_key(
                        KeyCode::ShiftLeft,
                        false,
                        false,
                        KeyCode::ShiftLeft,
                        Source::Pad,
                    )
                } else {
                    // As a pad's toggling stick click (`pad_action`).
                    self.toggles.pad_toggle(SNEAK, false);
                    self.sync_toggle(SNEAK);
                    self.key_pressed(KeyCode::ShiftLeft, KeyCode::ShiftLeft)
                }
            }
            Output::Press(Control::Sneak)
            | Output::Release(Control::Pause | Control::Back | Control::Sneak) => Ok(()),
            Output::Press(control) => self.touch_mapping(control, true),
            Output::Release(control) => self.touch_mapping(control, false),
            Output::Hotbar(slot) => {
                let key = HOTBAR_KEYS[usize::from(slot).min(8)];
                self.mapped_key(key, true, false, key, Source::Pad)?;
                self.mapped_key(key, false, false, key, Source::Pad)
            }
            Output::PointerMove(p) => {
                self.touch_pointer(event_loop, p, scale, None);
                Ok(())
            }
            Output::PointerPress(button, p) => {
                let button = touch_button(button);
                self.touch_pointer(event_loop, p, scale, Some((button, ElementState::Pressed)));
                Ok(())
            }
            Output::PointerRelease(button, p) => {
                let button = touch_button(button);
                self.touch_pointer(event_loop, p, scale, Some((button, ElementState::Released)));
                Ok(())
            }
            Output::ShiftClick(p) => {
                let modifiers = self.modifiers;
                self.modifiers = modifiers | ModifiersState::SHIFT;
                let left = Some((MouseButton::Left, ElementState::Pressed));
                self.touch_pointer(event_loop, p, scale, left);
                let left = Some((MouseButton::Left, ElementState::Released));
                self.touch_pointer(event_loop, p, scale, left);
                self.modifiers = modifiers;
                Ok(())
            }
            Output::Scroll(lines) => {
                if let Some(id) = self.window.as_ref().map(|w| w.id()) {
                    let event = WindowEvent::MouseWheel {
                        device_id: DeviceId::dummy(),
                        delta: MouseScrollDelta::LineDelta(0.0, lines),
                        phase: TouchPhase::Moved,
                    };
                    self.handle_window_event(event_loop, id, event, InputOrigin::User);
                }
                Ok(())
            }
        }
    }

    /// A control as its vanilla mapping's default key or mouse button, as
    /// `pad_action` does for a pad's buttons.
    fn touch_mapping(&mut self, control: Control, down: bool) -> Result<()> {
        let Some(action) = control.mapping() else {
            return Ok(());
        };
        let default = KeyBinds::default_of(action);
        if let Some(button) = mouse_button(default) {
            if self.title.is_none() && self.screen == Screen::Playing {
                self.play_mouse(button, down)?;
            }
            Ok(())
        } else if let Some(key) = key_code(default) {
            self.mapped_key(key, down, false, key, Source::Pad)
        } else {
            Ok(())
        }
    }

    /// The pointer to GUI point `p`, then a button change there.
    fn touch_pointer(
        &mut self,
        event_loop: &dyn EventLoopCtl,
        p: [f32; 2],
        scale: f32,
        then: Option<(MouseButton, ElementState)>,
    ) {
        let Some(id) = self.window.as_ref().map(|w| w.id()) else {
            return;
        };
        let device_id = DeviceId::dummy();
        let position = PhysicalPosition::new(f64::from(p[0] * scale), f64::from(p[1] * scale));
        self.handle_window_event(
            event_loop,
            id,
            WindowEvent::CursorMoved {
                device_id,
                position,
            },
            InputOrigin::User,
        );
        if let Some((button, state)) = then {
            self.handle_window_event(
                event_loop,
                id,
                WindowEvent::MouseInput {
                    device_id,
                    state,
                    button,
                },
                InputOrigin::User,
            );
        }
    }

    /// The controls over everything else: the buttons and the stick while
    /// playing, the back button over in-game screens.
    pub(super) fn draw_touch(&self, batch: &mut HudBatch) {
        if !enabled() {
            return;
        }
        let Some((layout, _)) = self.touch_layout() else {
            return;
        };
        let atlas = &self.gui.atlas;
        // The alphas below are the default opacity's (0.5).
        let fade = 2.0 * self.settings.controller.touch.opacity;
        match self.touch_mode() {
            Mode::Play => {
                let swimming = self
                    .player
                    .as_ref()
                    .is_some_and(|p| p.fluids.was_touching_water);
                for control in Control::PLAY {
                    if !layout.shown(control) {
                        continue;
                    }
                    let held = self.touch.input.holding(control)
                        || (control == Control::Sneak && self.toggles.is_down(SNEAK));
                    if let Some(r) = layout.rect(control) {
                        let looks = bedrock_sprite(control, layout.flying, swimming);
                        if !draw_bedrock(batch, atlas, r, control, looks, held, fade) {
                            draw_button(batch, atlas, r, control, held, fade);
                        }
                    }
                }
                // The stick rests at the bottom left, as Bedrock's does.
                let (origin, knob, radius) = match self.touch.input.stick_at() {
                    Some((origin, finger, radius)) => {
                        let d = (finger[0] - origin[0]).hypot(finger[1] - origin[1]);
                        let k = if d > radius { radius / d } else { 1.0 };
                        let knob = [
                            origin[0] + (finger[0] - origin[0]) * k,
                            origin[1] + (finger[1] - origin[1]) * k,
                        ];
                        (origin, knob, radius)
                    }
                    None if !layout.minimal => {
                        let home = layout.stick_home();
                        (home, home, layout.stick_radius())
                    }
                    None => return,
                };
                if !draw_bedrock_stick(batch, atlas, origin, knob, radius, fade) {
                    disc(batch, atlas, origin, radius, faded(0, 70, fade));
                    disc(batch, atlas, knob, radius * 0.45, faded(255, 110, fade));
                }
            }
            Mode::Menu { back: true } => {
                if let Some(r) = layout.rect(Control::Back) {
                    draw_button(batch, atlas, r, Control::Back, false, fade);
                }
            }
            Mode::Menu { back: false } => {}
        }
    }
}

impl Loading {
    /// `WindowEvent::Touch` on the startup menu's title; true when the touch
    /// controls took it. Its taps, drags and scrolls reach `menu_event` as
    /// the mouse's events, as `Game`'s menus get them.
    pub(super) fn touch_event(&mut self, event_loop: &dyn EventLoopCtl, touch: &Touch) -> bool {
        if !enabled() {
            return false;
        }
        let Some((layout, scale)) = self.touch_layout() else {
            return true;
        };
        let pos = [
            touch.location.x as f32 / scale,
            touch.location.y as f32 / scale,
        ];
        self.touch.input.set_mode(LOADING_MODE);
        self.touch
            .input
            .event(touch.id, phase(touch.phase), pos, Instant::now(), &layout);
        self.touch_apply(event_loop, scale);
        true
    }

    /// Once per loop iteration: long presses.
    pub(super) fn touch_frame(&mut self, event_loop: &dyn EventLoopCtl) {
        if !enabled() {
            return;
        }
        let Some((_, scale)) = self.touch_layout() else {
            return;
        };
        self.touch.input.set_mode(LOADING_MODE);
        self.touch.input.frame(Instant::now());
        self.touch_apply(event_loop, scale);
    }

    fn touch_layout(&self) -> Option<(Layout, f32)> {
        let r = self.renderer.as_ref()?;
        let batch = HudBatch::new([r.config.width, r.config.height]);
        let layout = Layout {
            width: batch.width,
            height: batch.height,
            unit: JUMP_SIDE * batch.height * self.settings.controller.touch.scale(),
            inset: 0.0,
            left_handed: false,
            minimal: false,
            flying: false,
        };
        Some((layout, batch.scale))
    }

    fn touch_apply(&mut self, event_loop: &dyn EventLoopCtl, scale: f32) {
        let device_id = DeviceId::dummy();
        let at = |p: [f32; 2]| WindowEvent::CursorMoved {
            device_id,
            position: PhysicalPosition::new(f64::from(p[0] * scale), f64::from(p[1] * scale)),
        };
        let mouse = |button, state| WindowEvent::MouseInput {
            device_id,
            state,
            button,
        };
        let left = MouseButton::Left;
        for out in self.touch.input.take() {
            let events = match out {
                Output::PointerMove(p) => vec![at(p)],
                Output::PointerPress(b, p) => {
                    vec![at(p), mouse(touch_button(b), ElementState::Pressed)]
                }
                Output::PointerRelease(b, p) => {
                    vec![at(p), mouse(touch_button(b), ElementState::Released)]
                }
                // The title has nothing a Shift click changes: a plain click.
                Output::ShiftClick(p) => vec![
                    at(p),
                    mouse(left, ElementState::Pressed),
                    mouse(left, ElementState::Released),
                ],
                Output::Scroll(lines) => vec![WindowEvent::MouseWheel {
                    device_id,
                    delta: MouseScrollDelta::LineDelta(0.0, lines),
                    phase: TouchPhase::Moved,
                }],
                // Menus without a back button have no controls.
                Output::Press(_) | Output::Release(_) | Output::Hotbar(_) => vec![],
            };
            for event in events {
                // As `window_event`: the menu's error ends the loading.
                if let Err(error) = self.menu_event(event_loop, event) {
                    self.error = Some(error);
                    event_loop.exit();
                    return;
                }
            }
        }
    }
}

/// The startup menu has only the title's screens, which close themselves.
const LOADING_MODE: Mode = Mode::Menu { back: false };

fn phase(phase: TouchPhase) -> Phase {
    match phase {
        TouchPhase::Started => Phase::Started,
        TouchPhase::Moved => Phase::Moved,
        TouchPhase::Ended => Phase::Ended,
        TouchPhase::Cancelled => Phase::Cancelled,
    }
}

fn touch_button(button: Button) -> MouseButton {
    match button {
        Button::Left => MouseButton::Left,
        Button::Right => MouseButton::Right,
    }
}

/// Grey `v` at `alpha` scaled by `fade`.
fn faded(v: u8, alpha: u8, fade: f32) -> u32 {
    rgba(v, v, v, (f32::from(alpha) * fade).round().min(255.0) as u8)
}

/// A Bedrock texture under `textures/gui/bedrock` (`ui/jump`).
fn bedrock(atlas: &HudAtlas, name: &str) -> Option<hud::Rect> {
    atlas.sprites.get(&format!("gui/bedrock/{name}")).copied()
}

/// The `[x, y, w, h]` texels `region` of `sprite` over `r`, sampled at the
/// texel centres so a scaled-up button takes nothing of its atlas neighbours.
fn blit(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    sprite: hud::Rect,
    region: Rect,
    r: Rect,
    color: u32,
) {
    let [sx, sy, sw, sh] = region;
    let (du, dv) = (
        (sprite.u1 - sprite.u0) / sprite.w as f32,
        (sprite.v1 - sprite.v0) / sprite.h as f32,
    );
    let (hu, hv) = (
        0.5 / atlas.width.max(1) as f32,
        0.5 / atlas.height.max(1) as f32,
    );
    let uv = hud::Rect {
        u0: sprite.u0 + sx * du + hu,
        v0: sprite.v0 + sy * dv + hv,
        u1: sprite.u0 + (sx + sw) * du - hu,
        v1: sprite.v0 + (sy + sh) * dv - hv,
        ..sprite
    };
    batch.rect_uv(r[0], r[1], r[2], r[3], uv, color);
}

/// A whole sprite over `r`, or fitted inside it keeping its shape (the
/// pause bars, the inventory's dots).
fn blit_all(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    sprite: hud::Rect,
    r: Rect,
    fit: bool,
    color: u32,
) {
    let (sw, sh) = (sprite.w as f32, sprite.h as f32);
    let r = if fit {
        let k = (r[2] / sw).min(r[3] / sh);
        let (w, h) = (sw * k, sh * k);
        [r[0] + (r[2] - w) / 2.0, r[1] + (r[3] - h) / 2.0, w, h]
    } else {
        r
    };
    blit(batch, atlas, sprite, [0.0, 0.0, sw, sh], r, color);
}

/// The Bedrock texture a button wears (its pressed look adds `_pressed`);
/// `None` for those drawn from pieces of `gui/gui`.
fn bedrock_sprite(control: Control, flying: bool, swimming: bool) -> Option<&'static str> {
    Some(match control {
        Control::Jump if swimming => "gui/controls/waterascend",
        Control::Descend if swimming => "gui/controls/waterdescend",
        Control::Jump if flying => "gui/controls/flyingascend",
        Control::Descend => "gui/controls/flyingdescend",
        Control::Jump => "ui/jump",
        Control::Sneak => "ui/sneak",
        Control::Sprint => "ui/sprint",
        Control::Attack => "ui/attack",
        Control::Interact => "ui/interact",
        _ => return None,
    })
}

/// A button in Bedrock's textures; false when the resources lack them.
fn draw_bedrock(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    r: Rect,
    control: Control,
    looks: Option<&str>,
    held: bool,
    fade: f32,
) -> bool {
    let color = faded(255, 166, fade);
    if let Some(name) = looks {
        let pressed = held
            .then(|| bedrock(atlas, &format!("{name}_pressed")))
            .flatten();
        let Some(sprite) = pressed.or_else(|| bedrock(atlas, name)) else {
            return false;
        };
        blit_all(batch, atlas, sprite, r, false, color);
        return true;
    }
    let Some(gui) = bedrock(atlas, "gui/gui") else {
        return false;
    };
    // `gui/gui`'s chat button (18 x 18 at 200, 82): its frame is pause's too.
    const CHAT: Rect = [200.0, 82.0, 18.0, 18.0];
    let [x, y, w, h] = r;
    let tint = if held { faded(160, 166, fade) } else { color };
    match control {
        Control::Chat => blit(batch, atlas, gui, CHAT, r, tint),
        Control::Pause => {
            let Some(bars) = bedrock(atlas, "ui/pause_icon") else {
                return false;
            };
            // The chat button's frame without its bubble: nine pieces, the
            // middle one filled from a texel of its plain inside. (Covering the
            // bubble instead lets it show through: the controls are see-through.)
            let xs = [200.0, 202.0, 216.0, 218.0];
            let ys = [82.0, 84.0, 96.0, 100.0];
            let dx = [x, x + w * 2.0 / 18.0, x + w * 16.0 / 18.0, x + w];
            let dy = [y, y + h * 2.0 / 18.0, y + h * 14.0 / 18.0, y + h];
            for row in 0..3 {
                for col in 0..3 {
                    let source = if (row, col) == (1, 1) {
                        [202.0, 84.0, 1.0, 1.0]
                    } else {
                        [
                            xs[col],
                            ys[row],
                            xs[col + 1] - xs[col],
                            ys[row + 1] - ys[row],
                        ]
                    };
                    let to = [
                        dx[col],
                        dy[row],
                        dx[col + 1] - dx[col],
                        dy[row + 1] - dy[row],
                    ];
                    blit(batch, atlas, gui, source, to, tint);
                }
            }
            let icon = [x + w * 0.25, y + h * 0.2, w * 0.5, h * 0.5];
            blit_all(batch, atlas, bars, icon, true, tint);
        }
        Control::Inventory => {
            let (Some(knob), Some(dots)) = (
                bedrock(atlas, "ui/joystick_knob"),
                bedrock(atlas, "ui/more-dots"),
            ) else {
                return false;
            };
            blit_all(batch, atlas, knob, r, false, tint);
            let inner = [x + w * 0.2, y + h * 0.2, w * 0.6, h * 0.6];
            blit_all(batch, atlas, dots, inner, true, tint);
        }
        _ => return false,
    }
    true
}

/// Bedrock's stick: its frame around `origin`, the knob towards the finger
/// (half its reach, so it stays on the frame); false without the textures.
fn draw_bedrock_stick(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    origin: [f32; 2],
    knob: [f32; 2],
    radius: f32,
    fade: f32,
) -> bool {
    let (Some(frame), Some(ball)) = (
        bedrock(atlas, "ui/joystick_frame"),
        bedrock(atlas, "ui/joystick_knob"),
    ) else {
        return false;
    };
    let color = faded(255, 166, fade);
    let r = radius;
    blit_all(
        batch,
        atlas,
        frame,
        [origin[0] - r, origin[1] - r, 2.0 * r, 2.0 * r],
        false,
        color,
    );
    let c = [
        origin[0] + (knob[0] - origin[0]) * 0.5,
        origin[1] + (knob[1] - origin[1]) * 0.5,
    ];
    let k = r * 0.34;
    blit_all(
        batch,
        atlas,
        ball,
        [c[0] - k, c[1] - k, 2.0 * k, 2.0 * k],
        false,
        color,
    );
    true
}

/// A filled circle (a 20-gon).
fn disc(batch: &mut HudBatch, atlas: &HudAtlas, c: [f32; 2], r: f32, color: u32) {
    const SIDES: usize = 20;
    let at = |i: usize| {
        let a = i as f32 / SIDES as f32 * std::f32::consts::TAU;
        [c[0] + r * a.cos(), c[1] + r * a.sin()]
    };
    for i in 0..SIDES {
        let (a, b) = (at(i), at(i + 1));
        batch.fill_quad(atlas, [c, a, b, b], color);
    }
}

/// A line from `a` to `b`, `width` thick.
fn line(batch: &mut HudBatch, atlas: &HudAtlas, a: [f32; 2], b: [f32; 2], width: f32, color: u32) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy).max(f32::EPSILON);
    let (nx, ny) = (-dy / len * width / 2.0, dx / len * width / 2.0);
    batch.fill_quad(
        atlas,
        [
            [a[0] + nx, a[1] + ny],
            [b[0] + nx, b[1] + ny],
            [b[0] - nx, b[1] - ny],
            [a[0] - nx, a[1] - ny],
        ],
        color,
    );
}

/// A button: a dark square with a light glyph, brighter while held. The
/// glyphs are shapes, so nothing needs translating.
fn draw_button(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    r: Rect,
    control: Control,
    held: bool,
    fade: f32,
) {
    let [x, y, w, h] = r;
    let back = faded(if held { 255 } else { 0 }, 90, fade);
    batch.fill(atlas, x, y, w, h, back);
    let edge = faded(255, 60, fade);
    let t = (w / 24.0).max(0.5);
    batch.fill(atlas, x, y, w, t, edge);
    batch.fill(atlas, x, y + h - t, w, t, edge);
    batch.fill(atlas, x, y + t, t, h - 2.0 * t, edge);
    batch.fill(atlas, x + w - t, y + t, t, h - 2.0 * t, edge);
    let ink = faded(255, 210, fade);
    let (cx, cy, s) = (x + w / 2.0, y + h / 2.0, w);
    let p = |u: f32, v: f32| [cx + u * s, cy + v * s];
    let stroke = s * 0.09;
    match control {
        Control::Jump => batch.fill_quad(
            atlas,
            [p(0.0, -0.22), p(0.24, 0.14), p(-0.24, 0.14), p(0.0, -0.22)],
            ink,
        ),
        Control::Descend => batch.fill_quad(
            atlas,
            [p(0.0, 0.22), p(-0.24, -0.14), p(0.24, -0.14), p(0.0, 0.22)],
            ink,
        ),
        Control::Sneak => {
            // A chevron pointing down over the ground.
            line(batch, atlas, p(-0.2, -0.14), p(0.0, 0.06), stroke, ink);
            line(batch, atlas, p(0.0, 0.06), p(0.2, -0.14), stroke, ink);
            line(batch, atlas, p(-0.24, 0.22), p(0.24, 0.22), stroke, ink);
        }
        Control::Interact => {
            line(batch, atlas, p(0.0, -0.24), p(0.0, 0.24), stroke, ink);
            line(batch, atlas, p(-0.24, 0.0), p(0.24, 0.0), stroke, ink);
        }
        Control::Attack => {
            // Crossed blades.
            line(batch, atlas, p(-0.22, -0.22), p(0.22, 0.22), stroke, ink);
            line(batch, atlas, p(0.22, -0.22), p(-0.22, 0.22), stroke, ink);
        }
        Control::Inventory => {
            for u in [-0.22, 0.0, 0.22] {
                let d = s * 0.12;
                let [px, py] = p(u, 0.0);
                batch.fill(atlas, px - d / 2.0, py - d / 2.0, d, d, ink);
            }
        }
        Control::Pause => {
            line(batch, atlas, p(-0.1, -0.2), p(-0.1, 0.2), stroke * 1.3, ink);
            line(batch, atlas, p(0.1, -0.2), p(0.1, 0.2), stroke * 1.3, ink);
        }
        Control::Back => {
            line(
                batch,
                atlas,
                p(0.08, -0.2),
                p(-0.12, 0.0),
                stroke * 1.3,
                ink,
            );
            line(batch, atlas, p(-0.12, 0.0), p(0.08, 0.2), stroke * 1.3, ink);
        }
        Control::Chat => {
            // A speech bubble: an outlined box with a tail at the lower left.
            let (l, r, t, b) = (cx - s * 0.24, cx + s * 0.24, cy - 0.18 * s, cy + 0.1 * s);
            let k = stroke * 0.8;
            batch.fill(atlas, l, t, r - l, k, ink);
            batch.fill(atlas, l, b - k, r - l, k, ink);
            batch.fill(atlas, l, t, k, b - t, ink);
            batch.fill(atlas, r - k, t, k, b - t, ink);
            batch.fill_quad(
                atlas,
                [p(-0.14, 0.1), p(-0.02, 0.1), p(-0.16, 0.24), p(-0.14, 0.1)],
                ink,
            );
        }
        Control::Sprint => {
            // Two chevrons pointing ahead.
            for v in [-0.02, 0.16] {
                line(batch, atlas, p(-0.2, v), p(0.0, v - 0.18), stroke, ink);
                line(batch, atlas, p(0.0, v - 0.18), p(0.2, v), stroke, ink);
            }
        }
        Control::Use | Control::Drop => {}
    }
}
