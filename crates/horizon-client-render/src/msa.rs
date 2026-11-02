//! ios-port: signing in with a Microsoft account inside the game, for players with no launcher to
//! hand it a session (the iOS app). The launcher's device-code flow (launcher/src/account.rs,
//! docs/microsoft-account.md): a code the player enters at microsoft.com/link, then Xbox Live,
//! XSTS, the Minecraft token, ownership and profile. The result becomes `auth::current`, so
//! online-mode servers, chat signing and single player use it as they use the launcher's.
//!
//! The Microsoft refresh token goes to the platform's store (the iOS app registers its Keychain,
//! [`set_platform`]); without one the sign-in lasts until the game quits. Tokens are never logged,
//! printed or shown.
use crate::auth::{self, AuthSession};
use serde_json::{Value, json};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCOPE: &str = "XboxLive.signin offline_access";
/// The Minecraft token is renewed when it has less than this left.
const RENEW_BEFORE: u64 = 3600;

/// The Azure app the sign-in goes through: the run-time variable, else the one built in.
pub fn client_id() -> Option<String> {
    std::env::var("HORIZON_MSA_CLIENT_ID")
        .ok()
        .or_else(|| option_env!("HORIZON_MSA_CLIENT_ID").map(str::to_owned))
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
}

/// Whether this build can sign in: the Account page shows only then.
pub fn available() -> bool {
    client_id().is_some()
}

/// What the platform does for the sign-in: keep the refresh token and open the link.
pub struct Platform {
    pub load: fn() -> Option<String>,
    pub save: fn(&str),
    pub delete: fn(),
    /// Open the verification page, the code ready to paste.
    pub open: fn(url: &str, code: &str),
}
static PLATFORM: OnceLock<Platform> = OnceLock::new();

pub fn set_platform(platform: Platform) {
    let _ = PLATFORM.set(platform);
}

/// Where the requests go: the real services, or a mock in tests.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub microsoft: String,
    pub xbox_user: String,
    pub xsts: String,
    pub minecraft: String,
}
impl Endpoints {
    pub fn live() -> Self {
        Self {
            microsoft: "https://login.microsoftonline.com/consumers/oauth2/v2.0".into(),
            xbox_user: "https://user.auth.xboxlive.com/user/authenticate".into(),
            xsts: "https://xsts.auth.xboxlive.com/xsts/authorize".into(),
            minecraft: "https://api.minecraftservices.com".into(),
        }
    }
}

/// Why a sign-in failed: a lang key (`horizon.account.error.*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fail(pub &'static str);

/// What the Account page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    SignedOut,
    /// Asking Microsoft for a code, or, after it, signing in to Xbox and Minecraft.
    Working,
    /// The player enters `code` at `url`.
    Code {
        url: String,
        code: String,
    },
    SignedIn(String),
    Failed(Fail),
}

struct State {
    status: Status,
    /// Bumped by every sign-in, cancel and sign-out: a worker whose number is gone stops.
    run: u64,
    /// The code shown, for Open link.
    code: Option<DeviceCode>,
}
static STATE: Mutex<State> = Mutex::new(State {
    status: Status::SignedOut,
    run: 0,
    code: None,
});

/// The page's status. A session the launcher handed over shows as signed in.
pub fn status() -> Status {
    let status = STATE.lock().unwrap().status.clone();
    match (status, auth::current()) {
        (Status::SignedOut, Some(session)) => Status::SignedIn(session.name.clone()),
        (status, _) => status,
    }
}

fn set_status(run: u64, status: Status) -> bool {
    let mut state = STATE.lock().unwrap();
    if state.run != run {
        return false;
    }
    state.status = status;
    true
}

fn next_run(status: Status) -> u64 {
    let mut state = STATE.lock().unwrap();
    state.run += 1;
    state.status = status;
    state.code = None;
    state.run
}

/// Start a sign-in: the page shows the code once Microsoft gives one.
pub fn sign_in() {
    let Some(client) = client_id() else {
        return;
    };
    let run = next_run(Status::Working);
    spawn(move || {
        let ep = Endpoints::live();
        let cancelled = || STATE.lock().unwrap().run != run;
        let result = device_code(&ep, &client).and_then(|code| {
            {
                let mut state = STATE.lock().unwrap();
                if state.run != run {
                    return Err(Fail("horizon.account.error.cancelled"));
                }
                state.status = Status::Code {
                    url: code.verification_uri.clone(),
                    code: code.user_code.clone(),
                };
                state.code = Some(code.clone());
            }
            let tokens = wait_for_sign_in(&ep, &client, &code, &cancelled)?;
            set_status(run, Status::Working);
            finish_sign_in(&ep, tokens)
        });
        signed_in(&ep, &client, run, result);
    });
}

/// Sign in again with the kept refresh token, at start.
pub fn restore() {
    let (Some(client), Some(refresh)) = (client_id(), PLATFORM.get().and_then(|p| (p.load)()))
    else {
        return;
    };
    let run = next_run(Status::Working);
    spawn(move || {
        let ep = Endpoints::live();
        let result = refresh_ms(&ep, &client, &refresh).and_then(|t| finish_sign_in(&ep, t));
        signed_in(&ep, &client, run, result);
    });
}

/// Stop waiting for the player to finish in the browser.
pub fn cancel() {
    next_run(Status::SignedOut);
}

/// Forget the account: offline from here on, the kept refresh token deleted.
pub fn sign_out() {
    next_run(Status::SignedOut);
    auth::clear_current();
    if let Some(platform) = PLATFORM.get() {
        (platform.delete)();
    }
}

/// Open the page that asks for the code shown, where the platform can.
pub fn open_link() {
    let code = STATE.lock().unwrap().code.clone();
    if let (Some(code), Some(platform)) = (code, PLATFORM.get()) {
        (platform.open)(&code.verification_uri, &code.user_code);
    }
}

/// Whether Open link can open the page here (the platform registered how).
pub fn can_open_link() -> bool {
    PLATFORM.get().is_some()
}

/// The Account page's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SignIn,
    Cancel,
    SignOut,
    OpenLink,
}
pub fn act(action: Action) {
    match action {
        Action::SignIn => sign_in(),
        Action::Cancel => cancel(),
        Action::SignOut => sign_out(),
        Action::OpenLink => open_link(),
    }
}

fn spawn(work: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new()
        .name("Microsoft sign-in".into())
        .spawn(work)
    {
        log::warn!("Horizon: could not start the sign-in: {e}");
    }
}

/// A sign-in's end: the session in use and the refresh token kept, then renewals until the run
/// ends. A refused refresh token signs out; no answer keeps trying.
fn signed_in(ep: &Endpoints, client: &str, run: u64, mut result: Result<Signed, Fail>) {
    loop {
        let signed = match result {
            Ok(signed) => signed,
            Err(fail) => {
                if fail.0 == "horizon.account.error.sign_in_again"
                    && let Some(platform) = PLATFORM.get()
                {
                    (platform.delete)();
                }
                let shown = if fail.0 == "horizon.account.error.cancelled" {
                    Status::SignedOut
                } else {
                    Status::Failed(fail)
                };
                set_status(run, shown);
                return;
            }
        };
        let renew_at = signed.expires_at.saturating_sub(RENEW_BEFORE);
        let refresh = signed.refresh.clone();
        {
            let mut state = STATE.lock().unwrap();
            if state.run != run {
                return;
            }
            state.status = Status::SignedIn(signed.session.name.clone());
            state.code = None;
            auth::set_current(signed.session);
        }
        auth::prepare_chat_keys(true);
        if let Some(platform) = PLATFORM.get() {
            (platform.save)(&refresh);
        }
        loop {
            while now() < renew_at {
                if STATE.lock().unwrap().run != run {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            match refresh_ms(ep, client, &refresh).and_then(|t| finish_sign_in(ep, t)) {
                Err(Fail("horizon.account.error.network")) => {
                    std::thread::sleep(Duration::from_secs(60));
                }
                other => {
                    result = other;
                    break;
                }
            }
        }
    }
}

// ----- The requests -----------------------------------------------------------------------------

fn request(method: &str, url: &str) -> ureq::Request {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent(&format!("Horizon/{}", horizon_core::release::VERSION))
        .build()
        .request(method, url)
}

enum Body<'a> {
    Form(Vec<(&'a str, &'a str)>),
    Json(Value),
}

/// The status and JSON body (null when there is none). No answer is a network failure.
fn send(request: ureq::Request, body: Option<Body>) -> Result<(u16, Value), Fail> {
    let result = match body {
        None => request.call(),
        Some(Body::Form(fields)) => request.send_form(&fields),
        Some(Body::Json(value)) => request
            .set("Accept", "application/json")
            .set("Content-Type", "application/json")
            .send_string(&value.to_string()),
    };
    let response = match result {
        Ok(r) | Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(_)) => return Err(Fail("horizon.account.error.network")),
    };
    let status = response.status();
    let text = response.into_string().unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Clone)]
struct DeviceCode {
    user_code: String,
    device_code: String,
    verification_uri: String,
    interval: u64,
    expires_at: u64,
}

struct MsTokens {
    access: String,
    refresh: String,
}

fn device_code(ep: &Endpoints, client: &str) -> Result<DeviceCode, Fail> {
    let (status, v) = send(
        request("POST", &format!("{}/devicecode", ep.microsoft)),
        Some(Body::Form(vec![("client_id", client), ("scope", SCOPE)])),
    )?;
    match (
        status,
        str_field(&v, "user_code"),
        str_field(&v, "device_code"),
    ) {
        (200, Some(user_code), Some(device_code)) => Ok(DeviceCode {
            user_code,
            device_code,
            verification_uri: str_field(&v, "verification_uri")
                .unwrap_or_else(|| "https://www.microsoft.com/link".into()),
            interval: v
                .get("interval")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .max(1),
            expires_at: now() + v.get("expires_in").and_then(Value::as_u64).unwrap_or(900),
        }),
        _ => Err(oauth_error(&v)),
    }
}

fn oauth_error(v: &Value) -> Fail {
    Fail(match v.get("error").and_then(Value::as_str) {
        Some("authorization_declined") => "horizon.account.error.declined",
        Some("expired_token" | "bad_verification_code") => "horizon.account.error.expired",
        Some("invalid_grant") => "horizon.account.error.sign_in_again",
        Some("invalid_client" | "unauthorized_client") => "horizon.account.error.client",
        _ => "horizon.account.error.microsoft",
    })
}

fn ms_tokens(status: u16, v: &Value) -> Result<MsTokens, Fail> {
    match (
        status,
        str_field(v, "access_token"),
        str_field(v, "refresh_token"),
    ) {
        (200, Some(access), Some(refresh)) => Ok(MsTokens { access, refresh }),
        _ => Err(oauth_error(v)),
    }
}

/// Poll the token endpoint as asked until the player finishes, the code expires or the run ends.
fn wait_for_sign_in(
    ep: &Endpoints,
    client: &str,
    code: &DeviceCode,
    cancelled: &dyn Fn() -> bool,
) -> Result<MsTokens, Fail> {
    let mut interval = code.interval;
    loop {
        let until = Instant::now() + Duration::from_secs(interval);
        while Instant::now() < until {
            if cancelled() {
                return Err(Fail("horizon.account.error.cancelled"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if now() > code.expires_at {
            return Err(Fail("horizon.account.error.expired"));
        }
        let (status, v) = send(
            request("POST", &format!("{}/token", ep.microsoft)),
            Some(Body::Form(vec![
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", client),
                ("device_code", &code.device_code),
            ])),
        )?;
        match v.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += 5,
            _ => return ms_tokens(status, &v),
        }
    }
}

fn refresh_ms(ep: &Endpoints, client: &str, refresh: &str) -> Result<MsTokens, Fail> {
    let (status, v) = send(
        request("POST", &format!("{}/token", ep.microsoft)),
        Some(Body::Form(vec![
            ("grant_type", "refresh_token"),
            ("client_id", client),
            ("refresh_token", refresh),
            ("scope", SCOPE),
        ])),
    )?;
    ms_tokens(status, &v)
}

/// The XSTS `XErr` codes, as the official launcher explains them.
fn xsts_error(code: u64) -> Fail {
    Fail(match code {
        2148916233 => "horizon.account.error.no_xbox_profile",
        2148916235 => "horizon.account.error.region",
        2148916236 | 2148916237 => "horizon.account.error.adult_verification",
        2148916238 => "horizon.account.error.child",
        _ => "horizon.account.error.xbox",
    })
}

/// A finished sign-in.
struct Signed {
    session: AuthSession,
    refresh: String,
    expires_at: u64,
}

/// From Microsoft tokens to a session: Xbox Live, XSTS, Minecraft, ownership, profile.
fn finish_sign_in(ep: &Endpoints, ms: MsTokens) -> Result<Signed, Fail> {
    let xbox = Fail("horizon.account.error.xbox");
    let (status, xbl) = send(
        request("POST", &ep.xbox_user),
        Some(Body::Json(json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={}", ms.access),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }))),
    )?;
    let xbl_token = str_field(&xbl, "Token")
        .filter(|_| status == 200)
        .ok_or(xbox)?;
    let (status, xsts) = send(
        request("POST", &ep.xsts),
        Some(Body::Json(json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }))),
    )?;
    if let Some(code) = xsts.get("XErr").and_then(Value::as_u64) {
        return Err(xsts_error(code));
    }
    let claims = xsts.pointer("/DisplayClaims/xui/0");
    let (Some(xsts_token), Some(uhs)) = (
        str_field(&xsts, "Token").filter(|_| status == 200),
        claims.and_then(|c| str_field(c, "uhs")),
    ) else {
        return Err(xbox);
    };
    let xuid = claims.and_then(|c| str_field(c, "xid"));
    let (token, expires_in) = minecraft_token(ep, &format!("XBL3.0 x={uhs};{xsts_token}"))?;
    let bearer = |path: &str| {
        request("GET", &format!("{}{path}", ep.minecraft))
            .set("Authorization", &format!("Bearer {token}"))
    };
    let (status, v) = send(bearer("/entitlements/mcstore"), None)?;
    if status != 200 {
        return Err(Fail("horizon.account.error.minecraft"));
    }
    let owns = v
        .get("items")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|i| {
                matches!(
                    i.get("name").and_then(Value::as_str),
                    Some("game_minecraft" | "product_minecraft")
                )
            })
        });
    if !owns {
        return Err(Fail("horizon.account.error.not_owned"));
    }
    let (status, v) = send(bearer("/minecraft/profile"), None)?;
    if status == 404 {
        return Err(Fail("horizon.account.error.no_profile"));
    }
    let (Some(uuid), Some(name)) = (
        str_field(&v, "id").as_deref().and_then(auth::parse_uuid),
        str_field(&v, "name"),
    ) else {
        return Err(Fail("horizon.account.error.minecraft"));
    };
    let mut session = AuthSession::new(&name, uuid, &token);
    session.xuid = xuid;
    Ok(Signed {
        session,
        refresh: ms.refresh,
        expires_at: now() + expires_in,
    })
}

/// The Minecraft token and its lifetime: `launcher/login`, then `login_with_xbox` (the
/// launcher's order, account.rs `minecraft_token`).
fn minecraft_token(ep: &Endpoints, xtoken: &str) -> Result<(String, u64), Fail> {
    let lifetime = |v: &Value| v.get("expires_in").and_then(Value::as_u64).unwrap_or(86400);
    if let Ok((200, v)) = send(
        request("POST", &format!("{}/launcher/login", ep.minecraft)),
        Some(Body::Json(
            json!({ "xtoken": xtoken, "platform": "PC_LAUNCHER" }),
        )),
    ) && let Some(token) = str_field(&v, "access_token")
    {
        return Ok((token, lifetime(&v)));
    }
    let (status, v) = send(
        request(
            "POST",
            &format!("{}/authentication/login_with_xbox", ep.minecraft),
        ),
        Some(Body::Json(json!({ "identityToken": xtoken }))),
    )?;
    match (status, str_field(&v, "access_token")) {
        (200, Some(token)) => Ok((token, lifetime(&v))),
        // New Azure apps are refused until Mojang approves them.
        (403, _) => Err(Fail("horizon.account.error.not_approved")),
        (429, _) => Err(Fail("horizon.account.error.too_many")),
        _ => Err(Fail("horizon.account.error.minecraft")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A local server answering each path with canned JSON, recording what it was asked.
    fn mock(
        routes: Vec<(&'static str, u16, Value)>,
    ) -> (Endpoints, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split(' ').nth(1).unwrap_or("").to_owned();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = header.to_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let _ = tx.send(format!("{path} {}", String::from_utf8_lossy(&body)));
                let (status, reply) = routes
                    .iter()
                    .find(|(p, ..)| path.ends_with(p))
                    .map_or((404, Value::Null), |(_, s, v)| (*s, v.clone()));
                let text = reply.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            }
        });
        let ep = Endpoints {
            microsoft: format!("{base}/ms"),
            xbox_user: format!("{base}/xbl"),
            xsts: format!("{base}/xsts"),
            minecraft: format!("{base}/mc"),
        };
        (ep, rx)
    }

    fn services(entitlements: Value, profile: (u16, Value)) -> Vec<(&'static str, u16, Value)> {
        vec![
            ("/xbl", 200, json!({"Token": "xbl-token"})),
            (
                "/xsts",
                200,
                json!({"Token": "xsts-token", "DisplayClaims": {"xui": [{"uhs": "hash", "xid": "2535"}]}}),
            ),
            (
                "/launcher/login",
                200,
                json!({"access_token": "mc-token", "expires_in": 86400}),
            ),
            ("/entitlements/mcstore", 200, entitlements),
            ("/minecraft/profile", profile.0, profile.1),
        ]
    }

    fn tokens() -> MsTokens {
        MsTokens {
            access: "ms-access".into(),
            refresh: "ms-refresh".into(),
        }
    }

    #[test]
    fn an_owner_signs_in_with_their_profile() {
        let (ep, asked) = mock(services(
            json!({"items": [{"name": "product_minecraft"}, {"name": "game_minecraft"}]}),
            (
                200,
                json!({"id": "069a79f444e94726a5befca90e38aaf5", "name": "Notch"}),
            ),
        ));
        let signed = finish_sign_in(&ep, tokens()).unwrap();
        assert_eq!(signed.session.name, "Notch");
        assert_eq!(signed.session.uuid, 0x069a79f444e94726a5befca90e38aaf5);
        assert_eq!(signed.session.xuid.as_deref(), Some("2535"));
        assert_eq!(signed.refresh, "ms-refresh");
        let asked: Vec<String> = asked.try_iter().collect();
        // The Microsoft token reaches Xbox Live; the XSTS token and hash reach Minecraft.
        assert!(asked[0].starts_with("/xbl ") && asked[0].contains("d=ms-access"));
        assert!(asked[2].contains("XBL3.0 x=hash;xsts-token"));
    }

    #[test]
    fn an_account_without_the_game_is_refused() {
        let (ep, _) = mock(services(json!({"items": []}), (404, Value::Null)));
        assert_eq!(
            finish_sign_in(&ep, tokens()).err(),
            Some(Fail("horizon.account.error.not_owned"))
        );
    }

    #[test]
    fn an_owner_without_a_profile_is_told_to_make_one() {
        let (ep, _) = mock(services(
            json!({"items": [{"name": "game_minecraft"}]}),
            (404, Value::Null),
        ));
        assert_eq!(
            finish_sign_in(&ep, tokens()).err(),
            Some(Fail("horizon.account.error.no_profile"))
        );
    }

    #[test]
    fn a_child_account_gets_the_family_message() {
        let mut routes = services(json!({}), (404, Value::Null));
        routes[1] = ("/xsts", 401, json!({"XErr": 2148916238u64}));
        let (ep, _) = mock(routes);
        assert_eq!(
            finish_sign_in(&ep, tokens()).err(),
            Some(Fail("horizon.account.error.child"))
        );
    }

    #[test]
    fn an_unapproved_app_is_named_when_both_logins_refuse() {
        let mut routes = services(json!({}), (404, Value::Null));
        routes[2] = ("/launcher/login", 403, Value::Null);
        routes.push(("/login_with_xbox", 403, Value::Null));
        let (ep, _) = mock(routes);
        assert_eq!(
            finish_sign_in(&ep, tokens()).err(),
            Some(Fail("horizon.account.error.not_approved"))
        );
    }

    #[test]
    fn the_code_flow_polls_until_the_player_finishes() {
        let (ep, asked) = mock(vec![
            (
                "/devicecode",
                200,
                json!({"user_code": "ABCD1234", "device_code": "dev", "verification_uri": "https://www.microsoft.com/link", "interval": 1, "expires_in": 900}),
            ),
            (
                "/token",
                200,
                json!({"access_token": "a", "refresh_token": "r"}),
            ),
        ]);
        let code = device_code(&ep, "client").unwrap();
        assert_eq!(code.user_code, "ABCD1234");
        let tokens = wait_for_sign_in(&ep, "client", &code, &|| false).unwrap();
        assert_eq!(
            (tokens.access.as_str(), tokens.refresh.as_str()),
            ("a", "r")
        );
        let token_request = asked
            .try_iter()
            .find(|r| r.starts_with("/ms/token"))
            .unwrap();
        assert!(token_request.contains("device_code=dev"));
    }

    #[test]
    fn a_declined_sign_in_ends_the_wait() {
        let (ep, _) = mock(vec![(
            "/token",
            400,
            json!({"error": "authorization_declined"}),
        )]);
        let code = DeviceCode {
            user_code: "C".into(),
            device_code: "d".into(),
            verification_uri: String::new(),
            interval: 1,
            expires_at: now() + 60,
        };
        assert_eq!(
            wait_for_sign_in(&ep, "client", &code, &|| false).err(),
            Some(Fail("horizon.account.error.declined"))
        );
    }
}
