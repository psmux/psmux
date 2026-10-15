use ratatui::layout::Rect;

use crate::cli::parse_target;
use crate::types::AppState;

pub const WINDOW_MINIMUM: u16 = crate::pane::MIN_PANE_DIM;
pub const WINDOW_MAXIMUM: u16 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowTarget {
    Current,
    Id(usize),
    Index(usize),
    Name(String),
    PaneId(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientSizeChoice {
    Largest,
    Smallest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResizeWindowRequest {
    pub target: WindowTarget,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub direction: Option<ResizeDirection>,
    pub adjustment: u32,
    pub client_size: Option<ClientSizeChoice>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeWindowResult {
    pub window_index: usize,
    pub window_id: usize,
    pub area: Rect,
}

fn parse_dimension(raw: &str, label: &str) -> Result<u16, String> {
    let value = raw
        .parse::<u32>()
        .map_err(|_| format!("{} invalid", label))?;
    if value < WINDOW_MINIMUM as u32 {
        return Err(format!("{} too small", label));
    }
    if value > WINDOW_MAXIMUM as u32 {
        return Err(format!("{} too large", label));
    }
    Ok(value as u16)
}

fn parse_adjustment(raw: &str) -> Result<u32, String> {
    let value = raw
        .parse::<u32>()
        .map_err(|_| "adjustment invalid".to_string())?;
    if value == 0 {
        return Err("adjustment too small".to_string());
    }
    if value > i32::MAX as u32 {
        return Err("adjustment too large".to_string());
    }
    Ok(value)
}

fn target_from_raw(raw: Option<&str>) -> Result<WindowTarget, String> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(WindowTarget::Current);
    };
    let parsed = parse_target(raw);
    if let Some(window) = parsed.window {
        return Ok(if parsed.window_is_id {
            WindowTarget::Id(window)
        } else {
            WindowTarget::Index(window)
        });
    }
    if let Some(name) = parsed.window_name {
        return Ok(WindowTarget::Name(name));
    }
    if parsed.pane_is_id {
        if let Some(pane) = parsed.pane {
            return Ok(WindowTarget::PaneId(pane));
        }
    }
    if raw == "." || raw.ends_with(':') || parsed.session.is_some() {
        return Ok(WindowTarget::Current);
    }
    Err(format!("can't find window: {}", raw))
}

pub fn parse_resize_window(
    args: &[&str],
    fallback_target: Option<&str>,
) -> Result<ResizeWindowRequest, String> {
    let mut target = fallback_target.map(str::to_string);
    let mut width = None;
    let mut height = None;
    let mut left = false;
    let mut right = false;
    let mut up = false;
    let mut down = false;
    let mut largest = false;
    let mut smallest = false;
    let mut positional = Vec::new();
    let mut i = 0;

    while i < args.len() {
        let token = args[i];
        if token == "--" {
            positional.extend(args[i + 1..].iter().copied());
            break;
        }
        if !token.starts_with('-') || token == "-" {
            positional.push(token);
            i += 1;
            continue;
        }
        if token.starts_with("--") {
            return Err(format!("unknown flag: {}", token));
        }

        let cluster = &token[1..];
        let bytes = cluster.as_bytes();
        let mut j = 0;
        while j < bytes.len() {
            let flag = bytes[j] as char;
            match flag {
                'a' => smallest = true,
                'A' => largest = true,
                'D' => down = true,
                'L' => left = true,
                'R' => right = true,
                'U' => up = true,
                't' | 'x' | 'y' => {
                    let attached = cluster[j + 1..]
                        .strip_prefix('=')
                        .unwrap_or(&cluster[j + 1..]);
                    let value = if attached.is_empty() {
                        i += 1;
                        args.get(i)
                            .copied()
                            .ok_or_else(|| format!("-{} expects an argument", flag))?
                    } else {
                        attached
                    };
                    match flag {
                        't' => target = Some(value.to_string()),
                        'x' => width = Some(parse_dimension(value, "width")?),
                        'y' => height = Some(parse_dimension(value, "height")?),
                        _ => unreachable!(),
                    }
                    break;
                }
                _ => return Err(format!("unknown flag: -{}", flag)),
            }
            j += 1;
        }
        i += 1;
    }

    if positional.len() > 1 {
        return Err("too many arguments".to_string());
    }
    let adjustment = positional
        .first()
        .map(|raw| parse_adjustment(raw))
        .transpose()?
        .unwrap_or(1);
    let direction = if left {
        Some(ResizeDirection::Left)
    } else if right {
        Some(ResizeDirection::Right)
    } else if up {
        Some(ResizeDirection::Up)
    } else if down {
        Some(ResizeDirection::Down)
    } else {
        None
    };
    let client_size = if largest {
        Some(ClientSizeChoice::Largest)
    } else if smallest {
        Some(ClientSizeChoice::Smallest)
    } else {
        None
    };

    Ok(ResizeWindowRequest {
        target: target_from_raw(target.as_deref())?,
        width,
        height,
        direction,
        adjustment,
        client_size,
    })
}

pub fn parse_control_client_size(
    spec: &str,
) -> Result<(Option<usize>, Option<(u16, u16)>), String> {
    let spec = spec.trim_matches(['"', '\'']);
    let (window_id, dimensions) = if let Some(rest) = spec.strip_prefix('@') {
        let (id, dimensions) = rest
            .split_once(':')
            .ok_or_else(|| "bad size argument".to_string())?;
        let id = id
            .parse::<usize>()
            .map_err(|_| "bad size argument".to_string())?;
        (Some(id), dimensions)
    } else {
        (None, spec)
    };
    if dimensions.is_empty() {
        if window_id.is_some() {
            return Ok((window_id, None));
        }
        return Err("bad size argument".to_string());
    }
    let (width, height) = dimensions
        .split_once(',')
        .or_else(|| dimensions.split_once('x'))
        .ok_or_else(|| "bad size argument".to_string())?;
    let parse = |raw: &str| -> Result<u16, String> {
        let value = raw
            .parse::<u32>()
            .map_err(|_| "bad size argument".to_string())?;
        if !(WINDOW_MINIMUM as u32..=WINDOW_MAXIMUM as u32).contains(&value) {
            return Err("size too small or too big".to_string());
        }
        Ok(value as u16)
    };
    Ok((window_id, Some((parse(width)?, parse(height)?))))
}

pub fn resolve_window(app: &AppState, target: &WindowTarget) -> Result<usize, String> {
    match target {
        WindowTarget::Current => app
            .windows
            .get(app.active_idx)
            .map(|_| app.active_idx)
            .ok_or_else(|| "can't find window".to_string()),
        WindowTarget::Id(id) => app
            .windows
            .iter()
            .position(|window| window.id == *id)
            .ok_or_else(|| format!("can't find window: @{}", id)),
        WindowTarget::Index(index) => app
            .win_pos(*index)
            .ok_or_else(|| format!("can't find window: {}", index)),
        WindowTarget::Name(name) => app
            .windows
            .iter()
            .position(|window| window.name == *name)
            .ok_or_else(|| format!("can't find window: {}", name)),
        WindowTarget::PaneId(id) => crate::tree::find_pane_by_id_global(app, *id)
            .map(|(window_index, _)| window_index)
            .ok_or_else(|| format!("can't find pane: %{}", id)),
    }
}

fn control_size_for_window(
    client: &crate::types::ControlClient,
    window_id: usize,
) -> Option<(u16, u16)> {
    client.window_sizes.get(&window_id).copied().or(client.size)
}

fn explicit_window_limit(app: &AppState, window_id: usize) -> Option<(u16, u16)> {
    app.control_clients
        .values()
        .filter_map(|client| client.window_sizes.get(&window_id).copied())
        .reduce(|left, right| (left.0.min(right.0), left.1.min(right.1)))
}

fn clamp_to_window_limit(app: &AppState, window_id: usize, size: (u16, u16)) -> (u16, u16) {
    explicit_window_limit(app, window_id)
        .map(|limit| (size.0.min(limit.0), size.1.min(limit.1)))
        .unwrap_or(size)
}

/// tmux resize.c ignore_client_size: a client that set `ignore-size` is left
/// out of the window size while any attached client has not set it (#825).
pub(crate) fn client_size_ignored(app: &AppState, client_id: u64) -> bool {
    app.ignore_size_clients.contains(&client_id)
        && app
            .client_registry
            .keys()
            .any(|id| !app.ignore_size_clients.contains(id))
}

fn client_sizes_for_window(app: &AppState, window_id: usize) -> Vec<(u64, (u16, u16))> {
    let mut sizes: Vec<(u64, (u16, u16))> = app
        .client_sizes
        .iter()
        .map(|(client_id, size)| (*client_id, *size))
        .collect();
    sizes.extend(
        app.control_clients
            .iter()
            .filter_map(|(client_id, client)| {
                control_size_for_window(client, window_id).map(|size| (*client_id, size))
            }),
    );
    sizes.retain(|(client_id, _)| !client_size_ignored(app, *client_id));
    sizes
}

fn default_client_size(app: &AppState, client_id: u64) -> Option<(u16, u16)> {
    if client_size_ignored(app, client_id) {
        return None;
    }
    app.client_sizes.get(&client_id).copied().or_else(|| {
        app.control_clients
            .get(&client_id)
            .and_then(|client| client.size)
    })
}

fn choose_client_size(
    app: &AppState,
    window_id: usize,
    choice: ClientSizeChoice,
) -> Option<(u16, u16)> {
    let sizes = client_sizes_for_window(app, window_id);
    if sizes.is_empty() {
        return None;
    }
    match choice {
        ClientSizeChoice::Largest => Some((
            sizes.iter().map(|(_, size)| size.0).max().unwrap(),
            sizes.iter().map(|(_, size)| size.1).max().unwrap(),
        )),
        ClientSizeChoice::Smallest => Some((
            sizes.iter().map(|(_, size)| size.0).min().unwrap(),
            sizes.iter().map(|(_, size)| size.1).min().unwrap(),
        )),
    }
}

fn effective_dynamic_size(app: &AppState, window: &crate::types::Window) -> Option<(u16, u16)> {
    let sizes = client_sizes_for_window(app, window.id);
    if sizes.is_empty() {
        return None;
    }
    let size = match window.window_size.as_deref().unwrap_or(&app.window_size) {
        "smallest" => Some((
            sizes.iter().map(|(_, size)| size.0).min().unwrap(),
            sizes.iter().map(|(_, size)| size.1).min().unwrap(),
        )),
        "largest" => Some((
            sizes.iter().map(|(_, size)| size.0).max().unwrap(),
            sizes.iter().map(|(_, size)| size.1).max().unwrap(),
        )),
        _ => app
            .latest_size_client_id
            .into_iter()
            .chain(app.latest_client_id)
            .find_map(|latest| {
                sizes
                    .iter()
                    .find(|(id, _)| *id == latest)
                    .map(|(_, size)| *size)
            })
            .or_else(|| {
                Some((
                    sizes.iter().map(|(_, size)| size.0).min().unwrap(),
                    sizes.iter().map(|(_, size)| size.1).min().unwrap(),
                ))
            }),
    };
    size.map(|size| clamp_to_window_limit(app, window.id, size))
}

fn effective_window_size(app: &AppState, window: &crate::types::Window) -> Option<(u16, u16)> {
    if window.window_size.as_deref().unwrap_or(&app.window_size) == "manual" {
        let requested = app
            .manual_window_sizes
            .get(&window.id)
            .copied()
            .unwrap_or((window.area.width, window.area.height));
        Some(clamp_to_window_limit(app, window.id, requested))
    } else {
        effective_dynamic_size(app, window)
    }
}

/// Record that `client_id` just sent a request: `window-size latest` follows
/// the client the user is actually using, not the one that happened to resize
/// last.  Returns true when the window geometry changed (the caller then
/// resizes the panes).
///
/// Only clients that reported a size can drive the geometry: one-shot CLI
/// clients (`psmux ls`) and CONTROL clients send requests constantly and have
/// no entry in `client_sizes`, so they are ignored.
pub fn note_client_activity(app: &mut AppState, client_id: u64) -> bool {
    if !app.client_sizes.contains_key(&client_id) {
        return false;
    }
    if app.latest_client_id == Some(client_id) && app.latest_size_client_id == Some(client_id) {
        return false; // already the latest: nothing to recompute
    }
    app.latest_client_id = Some(client_id);
    app.latest_size_client_id = Some(client_id);
    refresh_dynamic_window_sizes(app)
}

pub fn refresh_dynamic_window_sizes(app: &mut AppState) -> bool {
    let latest_size_is_live = app.latest_size_client_id.is_some_and(|client_id| {
        app.client_sizes.contains_key(&client_id)
            || app
                .control_clients
                .get(&client_id)
                .is_some_and(|client| client.size.is_some() || !client.window_sizes.is_empty())
    });
    if !latest_size_is_live {
        app.latest_size_client_id = app
            .client_sizes
            .keys()
            .copied()
            .chain(
                app.control_clients
                    .iter()
                    .filter_map(|(client_id, client)| {
                        (client.size.is_some() || !client.window_sizes.is_empty())
                            .then_some(*client_id)
                    }),
            )
            .max();
    }
    let latest_default_size = app
        .latest_size_client_id
        .and_then(|client_id| default_client_size(app, client_id))
        .or_else(|| {
            app.latest_client_id
                .and_then(|client_id| default_client_size(app, client_id))
        })
        .or_else(|| {
            app.client_sizes
                .iter()
                .filter(|(client_id, _)| !client_size_ignored(app, **client_id))
                .max_by_key(|(client_id, _)| *client_id)
                .map(|(_, size)| *size)
        })
        .or_else(|| {
            app.control_clients
                .iter()
                .filter(|(client_id, _)| !client_size_ignored(app, **client_id))
                .filter_map(|(client_id, client)| client.size.map(|size| (*client_id, size)))
                .max_by_key(|(client_id, _)| *client_id)
                .map(|(_, size)| size)
        });
    if let Some((width, height)) = latest_default_size {
        app.client_area = Rect::new(0, 0, width, height);
    }
    let missing_manual_sizes: Vec<(usize, (u16, u16))> = app
        .windows
        .iter()
        .filter(|window| window.window_size.as_deref().unwrap_or(&app.window_size) == "manual")
        .filter(|window| !app.manual_window_sizes.contains_key(&window.id))
        .map(|window| (window.id, (window.area.width, window.area.height)))
        .collect();
    for (window_id, size) in missing_manual_sizes {
        app.manual_window_sizes.insert(window_id, size);
    }
    let updates: Vec<(usize, Rect)> = app
        .windows
        .iter()
        .enumerate()
        .filter_map(|(index, window)| {
            effective_window_size(app, window)
                .map(|(width, height)| (index, Rect::new(0, 0, width, height)))
        })
        .collect();

    let mut any_changed = false;
    for (index, area) in updates {
        if app.windows[index].area != area {
            any_changed = true;
            app.windows[index].area = area;
            crate::tree::resize_window_panes(app, index, area);
        }
    }
    if let Some(active) = app.windows.get(app.active_idx) {
        app.last_window_area = active.area;
    }
    any_changed
}

pub fn apply_resize_window(
    app: &mut AppState,
    request: &ResizeWindowRequest,
) -> Result<ResizeWindowResult, String> {
    let window_index = resolve_window(app, &request.target)?;
    let window_id = app.windows[window_index].id;
    let mut width = request
        .width
        .unwrap_or(app.windows[window_index].area.width) as u32;
    let mut height = request
        .height
        .unwrap_or(app.windows[window_index].area.height) as u32;

    match request.direction {
        Some(ResizeDirection::Left) if width >= request.adjustment => {
            width -= request.adjustment;
        }
        Some(ResizeDirection::Right) => width = width.saturating_add(request.adjustment),
        Some(ResizeDirection::Up) if height >= request.adjustment => {
            height -= request.adjustment;
        }
        Some(ResizeDirection::Down) => height = height.saturating_add(request.adjustment),
        _ => {}
    }

    if let Some(choice) = request.client_size {
        let size = choose_client_size(app, window_id, choice)
            .unwrap_or((app.client_area.width, app.client_area.height));
        width = size.0 as u32;
        height = size.1 as u32;
    }

    let requested = (
        width.clamp(WINDOW_MINIMUM as u32, WINDOW_MAXIMUM as u32) as u16,
        height.clamp(WINDOW_MINIMUM as u32, WINDOW_MAXIMUM as u32) as u16,
    );
    app.manual_window_sizes.insert(window_id, requested);
    let actual = clamp_to_window_limit(app, window_id, requested);
    let area = Rect::new(0, 0, actual.0, actual.1);
    app.windows[window_index].area = area;
    app.windows[window_index].window_size = Some("manual".to_string());
    if window_index == app.active_idx {
        app.last_window_area = area;
    }
    crate::tree::resize_window_panes(app, window_index, area);

    Ok(ResizeWindowResult {
        window_index,
        window_id,
        area,
    })
}

pub fn set_active_window_size_mode(
    app: &mut AppState,
    value: Option<String>,
) -> Result<(), String> {
    if let Some(ref value) = value {
        if !matches!(value.as_str(), "latest" | "largest" | "smallest" | "manual") {
            return Err(format!("bad value: {}", value));
        }
    }
    let window_index = app.active_idx;
    if window_index >= app.windows.len() {
        return Err("can't find window".to_string());
    }
    app.windows[window_index].window_size = value;
    let is_manual = app.windows[window_index]
        .window_size
        .as_deref()
        .unwrap_or(&app.window_size)
        == "manual";
    if is_manual {
        let window = &app.windows[window_index];
        app.manual_window_sizes
            .entry(window.id)
            .or_insert((window.area.width, window.area.height));
    }
    if let Some((width, height)) = effective_window_size(app, &app.windows[window_index]) {
        let area = Rect::new(0, 0, width, height);
        let changed = app.windows[window_index].area != area;
        app.windows[window_index].area = area;
        app.last_window_area = area;
        if changed {
            crate::tree::resize_window_panes(app, window_index, area);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests-rs/test_resize_window_compat.rs"]
mod test_resize_window_compat;

#[cfg(test)]
#[path = "../tests-rs/test_client_size_after_reconnect.rs"]
mod test_client_size_after_reconnect;
