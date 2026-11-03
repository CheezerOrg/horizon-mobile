//! Touch controls (the iOS port): fingers on the screen as the player's
//! movement stick, view, buttons and hotbar while playing, and as the mouse
//! over menus. Pure logic in GUI pixels: the game feeds it the window's
//! touches, then acts on what it reports.
//!
//! Playing, Bedrock Edition's touch layout with its split controls (at the
//! spots horizon-android's Bedrock layout gives): the stick at the bottom
//! left, jump, sneak and sprint at the bottom right (ascend and descend while
//! flying or swimming), attack and interact buttons above them, pause and chat
//! at the top centre, and the inventory beside the hotbar. A finger on the
//! view only turns it: the buttons act at the crosshair. Holding a hotbar slot
//! drops its item. Menus: a tap is a left click,
//! a long press a right click, a drag a left drag, two fingers dragging
//! scroll and a two-finger tap is a Shift click.
use crate::gamepad;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

/// Held still this long, a finger right clicks in menus.
pub const LONG_PRESS: Duration = Duration::from_millis(450);
/// Held still this long on the view, a finger holds the use key (attack
/// when facing a block), as Bedrock Edition's touch controls do.
pub const LOOK_HOLD: Duration = Duration::from_millis(300);
/// Held this long on a hotbar slot, a finger drops the slot's item.
pub const HOLD_DROP: Duration = Duration::from_millis(500);
/// The game samples input once per tick: a timed press's release waits this
/// long so the press is seen.
pub const TAP_HOLD: Duration = Duration::from_millis(60);
/// The stick's deadzone, as a fraction of its radius.
pub const MOVE_DEADZONE: f32 = 0.15;
/// GUI pixels of two-finger travel per scroll-wheel line.
pub const SCROLL_STEP: f32 = 10.0;
/// Bedrock's jump button's side, as a fraction of the screen's height: the
/// `unit` the other controls are sized by.
pub const JUMP_SIDE: f32 = 0.16;
/// The vanilla hotbar's size in GUI pixels.
const HOTBAR_WIDTH: f32 = 182.0;
const HOTBAR_HEIGHT: f32 = 22.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// What a touch button (or gesture) stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Jump,
    /// A toggle: only its press counts.
    Sneak,
    Attack,
    Use,
    Inventory,
    Drop,
    /// Its button, or the stick pushed all the way forward.
    Sprint,
    /// Opens the pause menu while playing.
    Pause,
    /// Closes the open screen (Escape).
    Back,
    /// Opens the chat (and with it the keyboard).
    Chat,
    /// Sneak held, in place of the toggle while flying or swimming.
    Descend,
    /// Uses (the use key) at the crosshair: Bedrock's interact button.
    Interact,
}

impl Control {
    /// The buttons drawn while playing.
    pub const PLAY: [Control; 9] = [
        Control::Pause,
        Control::Chat,
        Control::Jump,
        Control::Sneak,
        Control::Descend,
        Control::Sprint,
        Control::Attack,
        Control::Interact,
        Control::Inventory,
    ];

    /// The vanilla key mapping it acts as; `None` for the ones the game
    /// handles itself.
    pub fn mapping(self) -> Option<&'static str> {
        Some(match self {
            Control::Jump => "key.jump",
            Control::Sneak | Control::Descend => "key.sneak",
            Control::Attack => "key.attack",
            Control::Use | Control::Interact => "key.use",
            Control::Inventory => "key.inventory",
            Control::Drop => "key.drop",
            Control::Sprint => "key.sprint",
            Control::Chat => "key.chat",
            Control::Pause | Control::Back => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// What the game does with the touches since the last `take`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Output {
    Press(Control),
    Release(Control),
    Hotbar(u8),
    /// The pointer moved to this GUI position.
    PointerMove([f32; 2]),
    PointerPress(Button, [f32; 2]),
    PointerRelease(Button, [f32; 2]),
    /// A left click with Shift held.
    ShiftClick([f32; 2]),
    /// Scroll-wheel lines, positive up.
    Scroll(f32),
}

/// Who the fingers drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Play,
    /// A screen is open; `back` shows a button that closes it.
    Menu {
        back: bool,
    },
}

/// `[x, y, width, height]` in GUI pixels.
pub type Rect = [f32; 4];

fn contains(r: Rect, p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Where the controls sit on a screen of `width` x `height` GUI pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    /// The jump button's side ([`JUMP_SIDE`] of the height, at the default
    /// size); the other controls are sized by it.
    pub unit: f32,
    /// Kept clear at the left and right edges (the notch, rounded corners).
    pub inset: f32,
    /// Mirrored: the stick on the right, the buttons on the left.
    pub left_handed: bool,
    /// The on-screen controls are off: only the pause button is left, so a
    /// phone with no keyboard can still reach the settings.
    pub minimal: bool,
    /// Flying or swimming: descend in place of sneak.
    pub flying: bool,
}

impl Layout {
    fn mirror(&self, r: Rect) -> Rect {
        if self.left_handed {
            [self.width - r[0] - r[2], r[1], r[2], r[3]]
        } else {
            r
        }
    }

    fn mirror_x(&self, x: f32) -> f32 {
        if self.left_handed { self.width - x } else { x }
    }

    /// A finger travelling less than this counts as held still.
    pub fn slop(&self) -> f32 {
        self.unit * 0.25
    }

    /// The stick's reach from where the finger landed.
    pub fn stick_radius(&self) -> f32 {
        self.unit
    }

    /// Whether the button is on the screen while playing.
    pub fn shown(&self, control: Control) -> bool {
        match control {
            Control::Pause => true,
            _ if self.minimal => false,
            Control::Chat
            | Control::Jump
            | Control::Sprint
            | Control::Attack
            | Control::Interact
            | Control::Inventory => true,
            Control::Sneak => !self.flying,
            Control::Descend => self.flying,
            Control::Use | Control::Drop | Control::Back => false,
        }
    }

    /// A square of `side` centred at fractions `fx`, `fy` of the screen
    /// (across the width inside the insets), kept on the screen and, where it
    /// would sit on Java's hotbar (wider than Bedrock's on a tablet), above it.
    fn spot(&self, fx: f32, fy: f32, side: f32) -> Rect {
        let (w, h, half) = (self.width, self.height, side / 2.0);
        let m = self.unit * 0.1;
        let cx = self.inset + fx * (w - 2.0 * self.inset);
        let cx = cx.clamp(self.inset + m + half, w - self.inset - m - half);
        let mut cy = (fy * h).clamp(m + half, h - m - half);
        let hotbar = w / 2.0 - HOTBAR_WIDTH / 2.0;
        // The hotbar and the inventory button beside it.
        let (left, right) = (hotbar, hotbar + HOTBAR_WIDTH + 2.0 + HOTBAR_HEIGHT);
        let top = h - HOTBAR_HEIGHT - m;
        if cx + half > left && cx - half < right && cy + half > top {
            cy = top - half;
        }
        [cx - half, cy - half, side, side]
    }

    /// The stick's centre while no finger holds it.
    pub fn stick_home(&self) -> [f32; 2] {
        let r = self.stick_radius();
        let [x, y, _, _] = self.spot(0.13, 0.76, 2.0 * r);
        [self.mirror_x(x + r), y + r]
    }

    /// The button's rectangle; `None` for controls with no button.
    pub fn rect(&self, control: Control) -> Option<Rect> {
        let (w, u) = (self.width, self.unit);
        let hotbar = w / 2.0 - HOTBAR_WIDTH / 2.0;
        // Bedrock's pause sits at the top centre, chat just left of it.
        let small = u * 0.47;
        let top = u * 0.1;
        let r = match control {
            Control::Pause => return Some([(w - small) / 2.0, top, small, small]),
            Control::Chat => return Some([(w - small) / 2.0 - top - small, top, small, small]),
            // Top left on either hand: toasts take the top right, and a
            // screen's title the top centre.
            Control::Back => return Some([self.inset + top, top, small, small]),
            Control::Jump => self.spot(0.92, 0.78, u),
            Control::Sneak | Control::Descend => self.spot(0.81, 0.86, u * 0.72),
            Control::Sprint => self.spot(0.72, 0.78, u * 0.66),
            // Bedrock's split controls: attack left of interact, a step lower.
            Control::Attack => self.spot(0.80, 0.64, u),
            Control::Interact => self.spot(0.92, 0.52, u),
            Control::Inventory => [
                hotbar + HOTBAR_WIDTH + 2.0,
                self.height - HOTBAR_HEIGHT,
                HOTBAR_HEIGHT,
                HOTBAR_HEIGHT,
            ],
            Control::Use | Control::Drop => return None,
        };
        Some(self.mirror(r))
    }

    /// The button under `p` in `mode`.
    pub fn button_at(&self, p: [f32; 2], mode: Mode) -> Option<Control> {
        match mode {
            Mode::Play => Control::PLAY
                .into_iter()
                .filter(|&c| self.shown(c))
                .find(|&c| self.rect(c).is_some_and(|r| contains(r, p))),
            Mode::Menu { back: true } => {
                (self.rect(Control::Back).is_some_and(|r| contains(r, p))).then_some(Control::Back)
            }
            Mode::Menu { back: false } => None,
        }
    }

    /// The hotbar slot under `p` (vanilla's 20-pixel slots, centred at the
    /// bottom).
    pub fn hotbar_slot(&self, p: [f32; 2]) -> Option<u8> {
        let x = p[0] - (self.width / 2.0 - HOTBAR_WIDTH / 2.0);
        (p[1] >= self.height - HOTBAR_HEIGHT - 1.0 && (0.0..HOTBAR_WIDTH).contains(&x))
            .then(|| ((x / 20.0) as u8).min(8))
    }

    /// The part of the screen where a finger becomes the stick.
    pub fn in_stick_zone(&self, p: [f32; 2]) -> bool {
        self.mirror_x(p[0]) < self.width * 0.4
    }
}

/// The touch settings (`horizon-touch.txt`, Horizon's own).
pub const FILE: &str = "horizon-touch.txt";

/// Whether this window takes touches as the touch controls: always on iOS,
/// elsewhere with `HORIZON_TOUCH` set (a touchscreen, or testing).
pub fn available() -> bool {
    cfg!(target_os = "ios") || std::env::var_os("HORIZON_TOUCH").is_some()
}

/// How the touch controls look and feel (the Touch Controls page).
#[derive(Clone, Debug, PartialEq)]
pub struct TouchSettings {
    /// The on-screen controls while playing; off leaves the pause button
    /// (for a keyboard or a controller). Menus take taps either way.
    pub enabled: bool,
    /// 0..1; 0.5 is the default size, 0 a little over half, 1 half again bigger.
    pub size: f32,
    /// 0..1: how solid the buttons are drawn.
    pub opacity: f32,
    /// 0..1; 0.5 is the default speed, 1 twice that.
    pub look_sensitivity: f32,
    pub left_handed: bool,
}

impl Default for TouchSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            size: 0.5,
            opacity: 0.5,
            look_sensitivity: 0.5,
            left_handed: false,
        }
    }
}

impl TouchSettings {
    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(dir.join(FILE))
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }
    /// Written once something differs from the defaults, so a desktop that
    /// never sees a touch gets no new file.
    pub fn save(&self, dir: &Path) {
        let path = dir.join(FILE);
        if *self != Self::default() || path.exists() {
            let _ = fs::write(path, self.serialize());
        }
    }
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            let flag = |d: bool| match value {
                "true" => true,
                "false" => false,
                _ => d,
            };
            let unit = |d: f32| {
                value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map_or(d, |v| v.clamp(0.0, 1.0))
            };
            match key.trim() {
                "enabled" => s.enabled = flag(s.enabled),
                "size" => s.size = unit(s.size),
                "opacity" => s.opacity = unit(s.opacity),
                "lookSensitivity" => s.look_sensitivity = unit(s.look_sensitivity),
                "leftHanded" => s.left_handed = flag(s.left_handed),
                _ => {}
            }
        }
        s
    }
    pub fn serialize(&self) -> String {
        format!(
            "enabled:{}\nsize:{}\nopacity:{}\nlookSensitivity:{}\nleftHanded:{}\n",
            self.enabled, self.size, self.opacity, self.look_sensitivity, self.left_handed,
        )
    }
    /// The buttons' size as a multiple of the default.
    pub fn scale(&self) -> f32 {
        0.6 + 0.8 * self.size
    }
    /// The view's turning speed as a multiple of the default.
    pub fn look_scale(&self) -> f32 {
        2.0 * self.look_sensitivity
    }
}

/// One touch setting, as the options screen shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchOpt {
    Enabled,
    Size,
    Opacity,
    LookSensitivity,
    LeftHanded,
}

impl TouchOpt {
    pub const ALL: [TouchOpt; 5] = [
        TouchOpt::Enabled,
        TouchOpt::LeftHanded,
        TouchOpt::Size,
        TouchOpt::Opacity,
        TouchOpt::LookSensitivity,
    ];
    /// Its caption's translation key (Horizon's lang, `assets/horizon/lang`).
    pub fn caption(self) -> &'static str {
        match self {
            TouchOpt::Enabled => "horizon.options.touch.controls",
            TouchOpt::Size => "horizon.options.touch.size",
            TouchOpt::Opacity => "horizon.options.touch.opacity",
            TouchOpt::LookSensitivity => "horizon.options.touch.look_sensitivity",
            TouchOpt::LeftHanded => "horizon.options.touch.left_handed",
        }
    }
    /// A slider's position, `None` for an on/off button.
    pub fn slider(self, s: &TouchSettings) -> Option<f32> {
        match self {
            TouchOpt::Size => Some(s.size),
            TouchOpt::Opacity => Some(s.opacity),
            TouchOpt::LookSensitivity => Some(s.look_sensitivity),
            TouchOpt::Enabled | TouchOpt::LeftHanded => None,
        }
    }
    /// The percentage a slider shows: of the default size and speed, or how
    /// solid the buttons are.
    pub fn percent(self, s: &TouchSettings) -> Option<f32> {
        match self {
            TouchOpt::Size => Some(s.scale()),
            TouchOpt::Opacity => Some(s.opacity),
            TouchOpt::LookSensitivity => Some(s.look_scale()),
            TouchOpt::Enabled | TouchOpt::LeftHanded => None,
        }
    }
    pub fn set_slider(self, s: &mut TouchSettings, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match self {
            TouchOpt::Size => s.size = v,
            TouchOpt::Opacity => s.opacity = v,
            TouchOpt::LookSensitivity => s.look_sensitivity = v,
            TouchOpt::Enabled | TouchOpt::LeftHanded => {}
        }
    }
    /// An on/off button's value.
    pub fn on(self, s: &TouchSettings) -> Option<bool> {
        match self {
            TouchOpt::Enabled => Some(s.enabled),
            TouchOpt::LeftHanded => Some(s.left_handed),
            _ => None,
        }
    }
    pub fn toggle(self, s: &mut TouchSettings) {
        match self {
            TouchOpt::Enabled => s.enabled = !s.enabled,
            TouchOpt::LeftHanded => s.left_handed = !s.left_handed,
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum MenuState {
    /// Not yet a tap, a drag or a long press.
    Pending,
    Dragging,
    /// The long press clicked; the rest of the touch does nothing.
    Done,
}

#[derive(Clone, Copy, Debug)]
enum LookState {
    /// Not yet a tap, a hold or a drag.
    Pending,
    /// It moved before it was held: it only turns the view.
    Dragging,
    /// The hold presses this control until the finger lifts.
    Holding(Control),
}

#[derive(Clone, Copy, Debug)]
enum Role {
    Button(Control),
    Hotbar {
        since: Instant,
        dropped: bool,
    },
    Stick {
        origin: [f32; 2],
        radius: f32,
    },
    /// Turns the view; tapped it attacks, held it uses (the other way round
    /// facing a block).
    Look {
        start: [f32; 2],
        since: Instant,
        state: LookState,
    },
    Menu {
        start: [f32; 2],
        since: Instant,
        state: MenuState,
    },
    /// One of the two fingers of a scroll or Shift click.
    Two,
    /// Nothing until it lifts (it began under another mode, or one too many).
    Ignored,
}

#[derive(Clone, Copy, Debug)]
struct Finger {
    id: u64,
    pos: [f32; 2],
    role: Role,
}

#[derive(Clone, Copy, Debug)]
struct TwoFingers {
    since: Instant,
    mid_y: f32,
    travel: f32,
    /// Where the first finger landed: the Shift click's spot.
    at: [f32; 2],
}

/// The fingers on the screen and what they have done since the game last
/// asked.
#[derive(Debug, Default)]
pub struct TouchInput {
    mode: Option<Mode>,
    fingers: Vec<Finger>,
    two: Option<TwoFingers>,
    out: Vec<Output>,
    look: [f32; 2],
    sprinting: bool,
    /// Releases of taps, held back until the time given.
    late: Vec<(Control, Instant)>,
    /// The crosshair is on a block (not an entity, not the air).
    facing_block: bool,
}

impl TouchInput {
    /// Switches who the fingers drive. On a change everything held is
    /// released and the fingers down stay ignored until they lift, so the tap
    /// that opened a screen clicks nothing in it.
    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode == Some(mode) {
            return;
        }
        self.mode = Some(mode);
        for finger in &mut self.fingers {
            match finger.role {
                Role::Button(c)
                | Role::Look {
                    state: LookState::Holding(c),
                    ..
                } => self.out.push(Output::Release(c)),
                Role::Menu {
                    state: MenuState::Dragging,
                    ..
                } => self
                    .out
                    .push(Output::PointerRelease(Button::Left, finger.pos)),
                _ => {}
            }
            finger.role = Role::Ignored;
        }
        self.two = None;
        if std::mem::take(&mut self.sprinting) {
            self.out.push(Output::Release(Control::Sprint));
        }
        for (control, _) in self.late.drain(..) {
            self.out.push(Output::Release(control));
        }
        self.look = [0.0, 0.0];
    }

    /// Whether the crosshair is on a block, for what the view finger's tap
    /// and hold do: tapped it uses and held it attacks (mines) there.
    pub fn set_facing_block(&mut self, facing: bool) {
        self.facing_block = facing;
    }

    /// A view finger is still to become a tap or a hold: the game should
    /// tell `set_facing_block` before the next `event` or `frame`.
    pub fn deciding(&self) -> bool {
        self.fingers.iter().any(|f| {
            matches!(
                f.role,
                Role::Look {
                    state: LookState::Pending,
                    ..
                }
            )
        })
    }

    /// A touch from the window, at `pos` in GUI pixels.
    pub fn event(&mut self, id: u64, phase: Phase, pos: [f32; 2], now: Instant, layout: &Layout) {
        match phase {
            Phase::Started => self.start(id, pos, now, layout),
            Phase::Moved => self.moved(id, pos, layout),
            Phase::Ended | Phase::Cancelled => {
                self.end(id, pos, now, layout, phase == Phase::Cancelled);
            }
        }
    }

    fn start(&mut self, id: u64, pos: [f32; 2], now: Instant, layout: &Layout) {
        // A lost end (the app went to the background) frees its id.
        self.fingers.retain(|f| f.id != id);
        let role = match self.mode {
            None => Role::Ignored,
            Some(Mode::Play) => self.play_role(pos, now, layout),
            Some(mode @ Mode::Menu { .. }) => self.menu_role(id, pos, now, layout, mode),
        };
        self.fingers.push(Finger { id, pos, role });
    }

    fn play_role(&mut self, pos: [f32; 2], now: Instant, layout: &Layout) -> Role {
        let taken = |f: &Finger, stick: bool| match f.role {
            Role::Stick { .. } => stick,
            Role::Look { .. } => !stick,
            _ => false,
        };
        if let Some(control) = layout.button_at(pos, Mode::Play) {
            self.out.push(Output::Press(control));
            Role::Button(control)
        } else if layout.minimal {
            Role::Ignored
        } else if let Some(slot) = layout.hotbar_slot(pos) {
            self.out.push(Output::Hotbar(slot));
            Role::Hotbar {
                since: now,
                dropped: false,
            }
        } else if layout.in_stick_zone(pos) {
            if self.fingers.iter().any(|f| taken(f, true)) {
                Role::Ignored
            } else {
                // On the drawn stick it starts from there, elsewhere where
                // the finger landed.
                let radius = layout.stick_radius();
                let home = layout.stick_home();
                let origin = if distance(pos, home) <= radius {
                    home
                } else {
                    pos
                };
                Role::Stick { origin, radius }
            }
        } else if self.fingers.iter().any(|f| taken(f, false)) {
            Role::Ignored
        } else {
            Role::Look {
                start: pos,
                since: now,
                state: LookState::Pending,
            }
        }
    }

    fn menu_role(
        &mut self,
        id: u64,
        pos: [f32; 2],
        now: Instant,
        layout: &Layout,
        mode: Mode,
    ) -> Role {
        if let Some(control) = layout.button_at(pos, mode) {
            self.out.push(Output::Press(control));
            return Role::Button(control);
        }
        if self.two.is_some() {
            return Role::Ignored;
        }
        let primary = self
            .fingers
            .iter_mut()
            .find(|f| matches!(f.role, Role::Menu { .. }) && f.id != id);
        match primary {
            Some(first) => {
                let Role::Menu {
                    start,
                    state: MenuState::Pending,
                    ..
                } = first.role
                else {
                    // A drag or a long press is already under way.
                    return Role::Ignored;
                };
                first.role = Role::Two;
                self.two = Some(TwoFingers {
                    since: now,
                    mid_y: (first.pos[1] + pos[1]) / 2.0,
                    travel: 0.0,
                    at: start,
                });
                Role::Two
            }
            None => {
                self.out.push(Output::PointerMove(pos));
                Role::Menu {
                    start: pos,
                    since: now,
                    state: MenuState::Pending,
                }
            }
        }
    }

    fn moved(&mut self, id: u64, pos: [f32; 2], layout: &Layout) {
        let Some(i) = self.fingers.iter().position(|f| f.id == id) else {
            return;
        };
        let old = self.fingers[i].pos;
        self.fingers[i].pos = pos;
        match &mut self.fingers[i].role {
            Role::Stick { origin, radius } => {
                // Past its reach the stick's centre follows the finger.
                let d = distance(pos, *origin);
                if d > *radius {
                    let k = (d - *radius) / d;
                    origin[0] += (pos[0] - origin[0]) * k;
                    origin[1] += (pos[1] - origin[1]) * k;
                }
            }
            Role::Look { start, state, .. } => {
                self.look[0] += pos[0] - old[0];
                self.look[1] += pos[1] - old[1];
                if matches!(state, LookState::Pending) && distance(pos, *start) > layout.slop() {
                    *state = LookState::Dragging;
                }
            }
            Role::Menu { start, state, .. } => match state {
                MenuState::Pending if distance(pos, *start) > layout.slop() => {
                    self.out.push(Output::PointerPress(Button::Left, *start));
                    self.out.push(Output::PointerMove(pos));
                    *state = MenuState::Dragging;
                }
                MenuState::Dragging => self.out.push(Output::PointerMove(pos)),
                _ => {}
            },
            Role::Two => {
                let Some(two) = self.two.as_mut() else { return };
                let ys: Vec<f32> = self
                    .fingers
                    .iter()
                    .filter(|f| matches!(f.role, Role::Two))
                    .map(|f| f.pos[1])
                    .collect();
                let mid = ys.iter().sum::<f32>() / ys.len() as f32;
                let dy = mid - two.mid_y;
                two.mid_y = mid;
                two.travel += dy.abs() + (pos[0] - old[0]).abs();
                if dy != 0.0 {
                    // The content follows the fingers: dragging down scrolls up.
                    self.out.push(Output::Scroll(dy / SCROLL_STEP));
                }
            }
            Role::Button(_) | Role::Hotbar { .. } | Role::Ignored => {}
        }
    }

    fn end(&mut self, id: u64, pos: [f32; 2], now: Instant, layout: &Layout, cancelled: bool) {
        let Some(i) = self.fingers.iter().position(|f| f.id == id) else {
            return;
        };
        let finger = self.fingers.remove(i);
        match finger.role {
            Role::Button(control)
            | Role::Look {
                state: LookState::Holding(control),
                ..
            } => self.out.push(Output::Release(control)),
            Role::Look {
                state: LookState::Pending,
                ..
            } if !cancelled => {
                let control = if self.facing_block {
                    Control::Use
                } else {
                    Control::Attack
                };
                self.out.push(Output::Press(control));
                self.late.push((control, now + TAP_HOLD));
            }
            Role::Menu { start, state, .. } => match state {
                MenuState::Pending if !cancelled => {
                    self.out.push(Output::PointerPress(Button::Left, start));
                    self.out.push(Output::PointerRelease(Button::Left, start));
                }
                MenuState::Dragging => self.out.push(Output::PointerRelease(Button::Left, pos)),
                _ => {}
            },
            Role::Two => {
                if let Some(two) = self.two.take()
                    && !cancelled
                    && two.travel <= layout.slop()
                    && now.duration_since(two.since) < LONG_PRESS
                {
                    self.out.push(Output::ShiftClick(two.at));
                }
                for f in &mut self.fingers {
                    if matches!(f.role, Role::Two) {
                        f.role = Role::Ignored;
                    }
                }
            }
            _ => {}
        }
    }

    /// Once per frame: the timed gestures, sprinting and late releases.
    pub fn frame(&mut self, now: Instant) {
        for finger in &mut self.fingers {
            match &mut finger.role {
                Role::Hotbar { since, dropped }
                    if !*dropped && now.duration_since(*since) >= HOLD_DROP =>
                {
                    *dropped = true;
                    self.out.push(Output::Press(Control::Drop));
                    self.late.push((Control::Drop, now + TAP_HOLD));
                }
                Role::Look { since, state, .. }
                    if matches!(state, LookState::Pending)
                        && now.duration_since(*since) >= LOOK_HOLD =>
                {
                    let control = if self.facing_block {
                        Control::Attack
                    } else {
                        Control::Use
                    };
                    self.out.push(Output::Press(control));
                    *state = LookState::Holding(control);
                }
                Role::Menu {
                    start,
                    since,
                    state,
                } if matches!(state, MenuState::Pending)
                    && now.duration_since(*since) >= LONG_PRESS =>
                {
                    self.out.push(Output::PointerPress(Button::Right, *start));
                    self.out.push(Output::PointerRelease(Button::Right, *start));
                    *state = MenuState::Done;
                }
                _ => {}
            }
        }
        let sprint = self
            .stick()
            .is_some_and(|v| gamepad::length(v) >= 0.95 && v[1] >= 0.8);
        if sprint != self.sprinting {
            self.sprinting = sprint;
            self.out.push(if sprint {
                Output::Press(Control::Sprint)
            } else {
                Output::Release(Control::Sprint)
            });
        }
        let out = &mut self.out;
        self.late.retain(|&(control, at)| {
            let due = at <= now;
            if due {
                out.push(Output::Release(control));
            }
            !due
        });
    }

    /// What happened since the last call.
    pub fn take(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.out)
    }

    /// The view finger's travel in GUI pixels since the last call.
    pub fn take_look(&mut self) -> [f32; 2] {
        std::mem::take(&mut self.look)
    }

    /// The stick's tilt, `[right, up]` with length at most 1, as a pad's stick.
    pub fn stick(&self) -> Option<[f32; 2]> {
        self.fingers.iter().find_map(|f| match f.role {
            Role::Stick { origin, radius } => {
                let v = [
                    (f.pos[0] - origin[0]) / radius,
                    (origin[1] - f.pos[1]) / radius,
                ];
                let len = gamepad::length(v);
                Some(if len > 1.0 {
                    [v[0] / len, v[1] / len]
                } else {
                    v
                })
            }
            _ => None,
        })
    }

    /// The stick's centre and the finger, for drawing it.
    pub fn stick_at(&self) -> Option<([f32; 2], [f32; 2], f32)> {
        self.fingers.iter().find_map(|f| match f.role {
            Role::Stick { origin, radius } => Some((origin, f.pos, radius)),
            _ => None,
        })
    }

    /// The move vector (`[left, forward]`), as a pad's left stick gives it.
    pub fn movement(&self) -> Option<[f32; 2]> {
        gamepad::move_vector(self.stick()?, MOVE_DEADZONE)
    }

    /// A finger holds this button down.
    pub fn holding(&self, control: Control) -> bool {
        self.fingers
            .iter()
            .any(|f| matches!(f.role, Role::Button(c) if c == control))
    }

    /// Sprint is held: its button, or the stick pushed all the way forward.
    pub fn sprinting(&self) -> bool {
        self.sprinting || self.holding(Control::Sprint)
    }

    /// No finger is on the screen.
    pub fn idle(&self) -> bool {
        self.fingers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAY: Mode = Mode::Play;
    const MENU: Mode = Mode::Menu { back: true };

    /// An iPhone in landscape (GUI scale 4) and an iPad (GUI scale 8).
    fn phone() -> Layout {
        Layout {
            width: 562.0,
            height: 287.0,
            unit: 287.0 * JUMP_SIDE,
            inset: 44.0,
            left_handed: false,
            minimal: false,
            flying: false,
        }
    }
    fn tablet() -> Layout {
        Layout {
            width: 342.0,
            height: 256.0,
            unit: 256.0 * JUMP_SIDE,
            inset: 0.0,
            left_handed: false,
            minimal: false,
            flying: false,
        }
    }

    struct Rig {
        input: TouchInput,
        layout: Layout,
        t0: Instant,
    }
    impl Rig {
        fn new(mode: Mode) -> Self {
            let mut input = TouchInput::default();
            input.set_mode(mode);
            Rig {
                input,
                layout: phone(),
                t0: Instant::now(),
            }
        }
        fn at(&self, ms: u64) -> Instant {
            self.t0 + Duration::from_millis(ms)
        }
        fn touch(&mut self, id: u64, phase: Phase, pos: [f32; 2], ms: u64) {
            let now = self.at(ms);
            self.input.event(id, phase, pos, now, &self.layout);
        }
        fn frame(&mut self, ms: u64) -> Vec<Output> {
            self.input.frame(self.at(ms));
            self.input.take()
        }
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
    }

    #[test]
    fn buttons_fit_the_screen_without_overlapping_each_other_or_the_hotbar() {
        let every = |base: Layout| {
            [false, true]
                .map(|flying| Layout { flying, ..base })
                .into_iter()
        };
        let left = Layout {
            left_handed: true,
            ..phone()
        };
        for layout in every(phone()).chain(every(tablet())).chain(every(left)) {
            let hotbar = [layout.width / 2.0 - 91.0, layout.height - 22.0, 182.0, 22.0];
            let r = layout.stick_radius();
            let [sx, sy] = layout.stick_home();
            let stick = [sx - r, sy - r, 2.0 * r, 2.0 * r];
            let rects: Vec<Rect> = Control::PLAY
                .iter()
                .filter(|&&c| layout.shown(c))
                .map(|&c| layout.rect(c).unwrap())
                .chain([stick])
                .collect();
            for (i, &a) in rects.iter().enumerate() {
                assert!(a[0] >= 0.0 && a[1] >= 0.0, "{layout:?} {a:?}");
                assert!(a[0] + a[2] <= layout.width && a[1] + a[3] <= layout.height);
                assert!(!overlap(a, hotbar), "{layout:?}: {a:?} covers the hotbar");
                for &b in &rects[i + 1..] {
                    assert!(!overlap(a, b), "{layout:?}: {a:?} and {b:?}");
                }
            }
        }
    }

    #[test]
    fn the_notch_inset_keeps_buttons_off_the_edge() {
        let layout = phone();
        let jump = layout.rect(Control::Jump).unwrap();
        assert!(jump[0] + jump[2] <= layout.width - layout.inset);
    }

    #[test]
    fn left_handed_swaps_the_stick_and_the_buttons() {
        let right = phone();
        let left = Layout {
            left_handed: true,
            ..right
        };
        let low_left = [60.0, 250.0];
        let low_right = [500.0, 250.0];
        assert!(right.in_stick_zone(low_left) && !right.in_stick_zone(low_right));
        assert!(left.in_stick_zone(low_right) && !left.in_stick_zone(low_left));
        assert!(right.rect(Control::Jump).unwrap()[0] > right.width / 2.0);
        assert!(left.rect(Control::Jump).unwrap()[0] < left.width / 2.0);
        assert!(right.stick_home()[0] < right.width / 2.0);
        assert!(left.stick_home()[0] > left.width / 2.0);
        // Pause stays at the top centre on either hand, chat just left of it.
        for layout in [right, left] {
            let pause = layout.rect(Control::Pause).unwrap();
            let chat = layout.rect(Control::Chat).unwrap();
            assert!(pause[0] < layout.width / 2.0 && pause[0] + pause[2] > layout.width / 2.0);
            assert!(chat[0] + chat[2] <= pause[0]);
        }
    }

    #[test]
    fn hotbar_slots_are_twenty_pixels_from_the_left_end() {
        let layout = phone();
        let x0 = layout.width / 2.0 - 91.0;
        let y = layout.height - 10.0;
        assert_eq!(layout.hotbar_slot([x0 + 1.0, y]), Some(0));
        assert_eq!(layout.hotbar_slot([x0 + 85.0, y]), Some(4));
        assert_eq!(layout.hotbar_slot([x0 + 181.0, y]), Some(8));
        assert_eq!(layout.hotbar_slot([x0 - 1.0, y]), None);
        assert_eq!(layout.hotbar_slot([x0 + 85.0, layout.height - 40.0]), None);
    }

    #[test]
    fn a_tap_on_a_menu_is_a_left_click_where_the_finger_landed() {
        let mut rig = Rig::new(MENU);
        rig.touch(1, Phase::Started, [100.0, 100.0], 0);
        rig.touch(1, Phase::Moved, [101.0, 100.0], 40);
        rig.touch(1, Phase::Ended, [101.0, 100.0], 80);
        assert_eq!(
            rig.frame(80),
            [
                Output::PointerMove([100.0, 100.0]),
                Output::PointerPress(Button::Left, [100.0, 100.0]),
                Output::PointerRelease(Button::Left, [100.0, 100.0]),
            ]
        );
    }

    #[test]
    fn a_long_press_right_clicks_once() {
        let mut rig = Rig::new(MENU);
        rig.touch(1, Phase::Started, [100.0, 100.0], 0);
        rig.input.take();
        assert_eq!(rig.frame(400), []);
        assert_eq!(
            rig.frame(460),
            [
                Output::PointerPress(Button::Right, [100.0, 100.0]),
                Output::PointerRelease(Button::Right, [100.0, 100.0]),
            ]
        );
        rig.touch(1, Phase::Moved, [150.0, 100.0], 500);
        rig.touch(1, Phase::Ended, [150.0, 100.0], 600);
        assert_eq!(rig.frame(600), []);
    }

    #[test]
    fn a_drag_presses_where_it_began_and_releases_where_it_ends() {
        let mut rig = Rig::new(MENU);
        rig.touch(1, Phase::Started, [100.0, 100.0], 0);
        rig.touch(1, Phase::Moved, [130.0, 100.0], 50);
        rig.touch(1, Phase::Moved, [160.0, 100.0], 100);
        rig.touch(1, Phase::Ended, [160.0, 100.0], 900);
        assert_eq!(
            rig.frame(900),
            [
                Output::PointerMove([100.0, 100.0]),
                Output::PointerPress(Button::Left, [100.0, 100.0]),
                Output::PointerMove([130.0, 100.0]),
                Output::PointerMove([160.0, 100.0]),
                Output::PointerRelease(Button::Left, [160.0, 100.0]),
            ]
        );
    }

    #[test]
    fn two_fingers_dragging_down_scroll_up_and_click_nothing() {
        let mut rig = Rig::new(MENU);
        rig.touch(1, Phase::Started, [100.0, 100.0], 0);
        rig.touch(2, Phase::Started, [140.0, 100.0], 20);
        rig.input.take();
        rig.touch(1, Phase::Moved, [100.0, 120.0], 40);
        rig.touch(2, Phase::Moved, [140.0, 120.0], 40);
        rig.touch(1, Phase::Ended, [100.0, 120.0], 100);
        rig.touch(2, Phase::Ended, [140.0, 120.0], 110);
        let out = rig.frame(900);
        let lines: f32 = out
            .iter()
            .map(|o| match o {
                Output::Scroll(l) => *l,
                other => panic!("{other:?}"),
            })
            .sum();
        assert!((lines - 20.0 / SCROLL_STEP).abs() < 1e-4, "{lines}");
    }

    #[test]
    fn a_two_finger_tap_shift_clicks_where_the_first_finger_landed() {
        let mut rig = Rig::new(MENU);
        rig.touch(1, Phase::Started, [100.0, 100.0], 0);
        rig.touch(2, Phase::Started, [140.0, 100.0], 20);
        rig.input.take();
        rig.touch(2, Phase::Ended, [140.0, 100.0], 120);
        rig.touch(1, Phase::Ended, [100.0, 100.0], 130);
        assert_eq!(rig.frame(900), [Output::ShiftClick([100.0, 100.0])]);
    }

    #[test]
    fn the_back_button_closes_menus_and_clicks_nothing_under_it() {
        let mut rig = Rig::new(MENU);
        let r = rig.layout.rect(Control::Back).unwrap();
        let p = [r[0] + 1.0, r[1] + 1.0];
        rig.touch(1, Phase::Started, p, 0);
        rig.touch(1, Phase::Ended, p, 50);
        assert_eq!(
            rig.frame(50),
            [Output::Press(Control::Back), Output::Release(Control::Back)]
        );
    }

    #[test]
    fn the_stick_finger_never_turns_the_view() {
        let mut rig = Rig::new(PLAY);
        rig.touch(1, Phase::Started, [80.0, 200.0], 0);
        // Pushed up and far to the right, over the view's half.
        rig.touch(1, Phase::Moved, [80.0, 150.0], 30);
        rig.touch(1, Phase::Moved, [400.0, 150.0], 60);
        assert_eq!(rig.input.take_look(), [0.0, 0.0]);
        // A second finger on the right turns it.
        rig.touch(2, Phase::Started, [450.0, 120.0], 70);
        rig.touch(2, Phase::Moved, [460.0, 115.0], 80);
        assert_eq!(rig.input.take_look(), [10.0, -5.0]);
        // The stick still points where finger 1 is, from a centre it dragged along.
        let v = rig.input.stick().unwrap();
        assert!(
            (gamepad::length(v) - 1.0).abs() < 1e-4 && v[0] > 0.9,
            "{v:?}"
        );
    }

    #[test]
    fn pushing_the_stick_up_walks_forward_and_a_nudge_does_nothing() {
        let mut rig = Rig::new(PLAY);
        let r = rig.layout.stick_radius();
        rig.touch(1, Phase::Started, [170.0, 120.0], 0);
        rig.touch(1, Phase::Moved, [170.0 + r * 0.1, 120.0], 10);
        assert_eq!(rig.input.movement(), None);
        rig.touch(1, Phase::Moved, [170.0, 120.0 - r * 0.6], 20);
        let [left, forward] = rig.input.movement().unwrap();
        assert!(
            left.abs() < 1e-4 && forward > 0.4 && forward < 0.6,
            "{forward}"
        );
        rig.touch(1, Phase::Ended, [170.0, 60.0], 30);
        assert_eq!(rig.input.movement(), None);
    }

    #[test]
    fn the_stick_all_the_way_forward_sprints() {
        let mut rig = Rig::new(PLAY);
        let r = rig.layout.stick_radius();
        rig.touch(1, Phase::Started, [170.0, 120.0], 0);
        rig.touch(1, Phase::Moved, [170.0, 120.0 - r * 0.5], 10);
        assert_eq!(rig.frame(10), []);
        rig.touch(1, Phase::Moved, [170.0, 120.0 - r * 1.5], 20);
        assert_eq!(rig.frame(20), [Output::Press(Control::Sprint)]);
        rig.touch(1, Phase::Ended, [170.0, 20.0], 30);
        assert_eq!(rig.frame(30), [Output::Release(Control::Sprint)]);
    }

    #[test]
    fn attack_and_interact_hold_their_keys_and_descend_replaces_sneak_while_flying() {
        let mut rig = Rig::new(PLAY);
        let centre = |r: Rect| [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0];
        let attack = centre(rig.layout.rect(Control::Attack).unwrap());
        let interact = centre(rig.layout.rect(Control::Interact).unwrap());
        rig.touch(1, Phase::Started, attack, 0);
        rig.touch(2, Phase::Started, interact, 0);
        assert_eq!(
            rig.frame(10),
            [
                Output::Press(Control::Attack),
                Output::Press(Control::Interact)
            ]
        );
        // Mining goes on while the finger stays down.
        assert_eq!(rig.frame(2000), []);
        rig.touch(1, Phase::Ended, attack, 2100);
        rig.touch(2, Phase::Ended, interact, 2100);
        assert_eq!(
            rig.frame(2100),
            [
                Output::Release(Control::Attack),
                Output::Release(Control::Interact)
            ]
        );
        let sneak = centre(rig.layout.rect(Control::Sneak).unwrap());
        assert_eq!(rig.layout.button_at(sneak, PLAY), Some(Control::Sneak));
        rig.layout.flying = true;
        assert_eq!(rig.layout.button_at(sneak, PLAY), Some(Control::Descend));
    }

    #[test]
    fn holding_a_hotbar_slot_selects_it_then_drops_one_item() {
        let mut rig = Rig::new(PLAY);
        let x0 = rig.layout.width / 2.0 - 91.0;
        rig.touch(1, Phase::Started, [x0 + 25.0, rig.layout.height - 5.0], 0);
        assert_eq!(rig.frame(10), [Output::Hotbar(1)]);
        assert_eq!(rig.frame(400), []);
        assert_eq!(rig.frame(510), [Output::Press(Control::Drop)]);
        assert_eq!(rig.frame(580), [Output::Release(Control::Drop)]);
        assert_eq!(rig.frame(2000), []);
        rig.touch(1, Phase::Ended, [x0 + 25.0, rig.layout.height - 5.0], 2100);
        assert_eq!(rig.frame(2100), []);
    }

    #[test]
    fn a_finger_on_the_drawn_stick_pushes_it_from_its_centre() {
        let mut rig = Rig::new(PLAY);
        let [hx, hy] = rig.layout.stick_home();
        let r = rig.layout.stick_radius();
        // Landing off-centre on the stick already tilts it.
        rig.touch(1, Phase::Started, [hx, hy - r * 0.8], 0);
        let [_, up] = rig.input.stick().unwrap();
        assert!((up - 0.8).abs() < 1e-4, "{up}");
        rig.touch(1, Phase::Ended, [hx, hy - r * 0.8], 10);
        // Away from it, the stick starts where the finger lands.
        rig.touch(2, Phase::Started, [hx + 2.5 * r, hy - 2.0 * r], 20);
        assert_eq!(rig.input.stick(), Some([0.0, 0.0]));
    }

    #[test]
    fn tapping_the_view_attacks_and_holding_it_uses_then_turns_it_while_held() {
        let mut rig = Rig::new(PLAY);
        rig.touch(1, Phase::Started, [400.0, 100.0], 0);
        rig.touch(1, Phase::Ended, [400.0, 100.0], 100);
        assert_eq!(rig.frame(100), [Output::Press(Control::Attack)]);
        assert_eq!(rig.frame(200), [Output::Release(Control::Attack)]);
        rig.touch(2, Phase::Started, [400.0, 100.0], 300);
        assert_eq!(rig.frame(500), []);
        assert_eq!(rig.frame(700), [Output::Press(Control::Use)]);
        rig.touch(2, Phase::Moved, [430.0, 100.0], 800);
        assert_eq!(rig.input.take_look(), [30.0, 0.0]);
        rig.touch(2, Phase::Ended, [430.0, 100.0], 900);
        assert_eq!(rig.frame(900), [Output::Release(Control::Use)]);
    }

    #[test]
    fn facing_a_block_a_tap_on_the_view_uses_and_a_hold_attacks() {
        let mut rig = Rig::new(PLAY);
        rig.input.set_facing_block(true);
        rig.touch(1, Phase::Started, [400.0, 100.0], 0);
        assert!(rig.input.deciding());
        rig.touch(1, Phase::Ended, [400.0, 100.0], 100);
        assert_eq!(
            rig.frame(200),
            [Output::Press(Control::Use), Output::Release(Control::Use)]
        );
        rig.touch(2, Phase::Started, [400.0, 100.0], 300);
        assert_eq!(rig.frame(700), [Output::Press(Control::Attack)]);
        assert!(!rig.input.deciding());
        rig.input.set_mode(MENU);
        assert_eq!(rig.input.take(), [Output::Release(Control::Attack)]);
    }

    #[test]
    fn a_drag_on_the_view_only_turns_it() {
        let mut rig = Rig::new(PLAY);
        rig.touch(1, Phase::Started, [400.0, 100.0], 0);
        rig.touch(1, Phase::Moved, [460.0, 120.0], 50);
        assert_eq!(rig.frame(1000), []);
        assert_eq!(rig.input.take_look(), [60.0, 20.0]);
        rig.touch(1, Phase::Ended, [460.0, 120.0], 1100);
        assert_eq!(rig.frame(1200), []);
    }

    #[test]
    fn buttons_hold_while_pressed_and_the_hotbar_selects_its_slot() {
        let mut rig = Rig::new(PLAY);
        let r = rig.layout.rect(Control::Jump).unwrap();
        let jump = [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0];
        rig.touch(1, Phase::Started, jump, 0);
        assert!(rig.input.holding(Control::Jump));
        rig.touch(1, Phase::Moved, [jump[0] - 60.0, jump[1]], 20);
        assert!(rig.input.holding(Control::Jump));
        rig.touch(1, Phase::Ended, jump, 40);
        let x0 = rig.layout.width / 2.0 - 91.0;
        rig.touch(2, Phase::Started, [x0 + 65.0, rig.layout.height - 5.0], 50);
        assert_eq!(
            rig.frame(60),
            [
                Output::Press(Control::Jump),
                Output::Release(Control::Jump),
                Output::Hotbar(3),
            ]
        );
        assert_eq!(rig.input.take_look(), [0.0, 0.0]);
    }

    #[test]
    fn a_screen_opening_releases_what_is_held_and_ignores_the_fingers_down() {
        let mut rig = Rig::new(PLAY);
        let r = rig.layout.rect(Control::Jump).unwrap();
        rig.touch(1, Phase::Started, [r[0] + 1.0, r[1] + 1.0], 0);
        rig.input.take();
        rig.input.set_mode(MENU);
        assert_eq!(rig.input.take(), [Output::Release(Control::Jump)]);
        rig.touch(1, Phase::Moved, [200.0, 100.0], 30);
        rig.touch(1, Phase::Ended, [200.0, 100.0], 60);
        assert_eq!(rig.frame(900), []);
    }

    #[test]
    fn touch_settings_round_trip_and_reject_what_is_out_of_range() {
        let s = TouchSettings {
            enabled: false,
            size: 0.25,
            opacity: 0.8,
            look_sensitivity: 1.0,
            left_handed: true,
        };
        assert_eq!(TouchSettings::parse(&s.serialize()), s);
        let odd = TouchSettings::parse("size:7\nopacity:nan\nleftHanded:maybe\nfuture:1\n");
        assert_eq!(odd.size, 1.0);
        assert_eq!(odd.opacity, TouchSettings::default().opacity);
        assert!(!odd.left_handed);
        // The defaults are the sizes and speed the controls were tuned at.
        assert_eq!(TouchSettings::default().scale(), 1.0);
        assert_eq!(TouchSettings::default().look_scale(), 1.0);
    }

    #[test]
    fn with_the_controls_off_only_the_pause_button_answers() {
        let mut rig = Rig::new(PLAY);
        rig.layout.minimal = true;
        let pause = rig.layout.rect(Control::Pause).unwrap();
        let jump = rig.layout.rect(Control::Jump).unwrap();
        rig.touch(1, Phase::Started, [jump[0] + 1.0, jump[1] + 1.0], 0);
        rig.touch(2, Phase::Started, [100.0, 200.0], 0);
        rig.touch(2, Phase::Moved, [130.0, 150.0], 10);
        rig.touch(3, Phase::Started, [400.0, 100.0], 0);
        rig.touch(3, Phase::Moved, [420.0, 100.0], 10);
        assert_eq!(rig.frame(500), []);
        assert_eq!(rig.input.take_look(), [0.0, 0.0]);
        assert_eq!(rig.input.movement(), None);
        rig.touch(4, Phase::Started, [pause[0] + 1.0, pause[1] + 1.0], 600);
        assert_eq!(rig.frame(610), [Output::Press(Control::Pause)]);
    }
}
