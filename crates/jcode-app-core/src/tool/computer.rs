//! Native macOS "computer use" tool.
//!
//! Lets the agent control the desktop GUI directly: take screenshots, move and
//! click the mouse, type text, send key chords, scroll, and inspect the
//! on-screen Accessibility (AX) UI tree. This is the macOS analog of the
//! `browser` tool, but for the whole desktop instead of a single web page.
//!
//! Synthetic input is generated through Core Graphics `CGEvent`s. Screenshots
//! shell out to the system `screencapture` binary. The Accessibility tree is
//! read via `System Events` (osascript).
//!
//! The tool only does anything useful on macOS; on other platforms every action
//! returns a clear "unsupported" error. Synthetic events additionally require
//! the host terminal/jcode binary to hold the macOS Accessibility permission,
//! and screenshots require the Screen Recording permission. When those are
//! missing the OS silently drops the events, so the tool surfaces actionable
//! guidance in its output.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

pub struct ComputerTool;

impl ComputerTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct ComputerInput {
    action: String,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    text: Option<String>,
    /// Key chord such as "cmd+space", "return", "esc", "ctrl+shift+t".
    #[serde(default)]
    keys: Option<String>,
    /// Scroll amounts (positive dy scrolls up, positive dx scrolls right).
    #[serde(default)]
    dx: Option<i32>,
    #[serde(default)]
    dy: Option<i32>,
    /// For `drag`: destination coordinates (x/y are the start point).
    #[serde(default)]
    to_x: Option<f64>,
    #[serde(default)]
    to_y: Option<f64>,
    /// For `ui`: maximum tree depth to walk (default 12).
    #[serde(default)]
    depth: Option<u32>,
}

#[async_trait]
impl Tool for ComputerTool {
    fn name(&self) -> &str {
        "computer"
    }

    fn description(&self) -> &str {
        "Control the macOS desktop GUI: screenshot the screen, move/click the \
         mouse, type text, press key chords, scroll, and read the on-screen \
         Accessibility (UI) tree of the focused app. Coordinates are in screen \
         points with the origin at the top-left. Requires macOS Accessibility \
         permission for input and Screen Recording permission for screenshots. \
         Use 'ui' to discover clickable elements and their coordinates before \
         clicking."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": [
                        "screenshot", "move", "click", "double_click",
                        "right_click", "drag", "type", "key", "scroll", "ui",
                        "cursor", "check_permissions"
                    ],
                    "description": "What to do. 'screenshot' captures the screen; \
                        'move' moves the cursor to (x,y); 'click'/'double_click'/'right_click' \
                        click at (x,y) (or current position if omitted); 'drag' drags from (x,y) \
                        to (to_x,to_y); 'type' types `text`; 'key' presses the `keys` chord \
                        (e.g. cmd+space); 'scroll' scrolls by (dx,dy) at (x,y); 'ui' dumps the \
                        focused app's Accessibility tree; 'cursor' reports the current cursor \
                        position; 'check_permissions' reports Accessibility/Screen-Recording status."
                },
                "x": { "type": "number", "description": "Screen X coordinate in points (origin top-left)." },
                "y": { "type": "number", "description": "Screen Y coordinate in points (origin top-left)." },
                "to_x": { "type": "number", "description": "Destination X in points for action='drag'." },
                "to_y": { "type": "number", "description": "Destination Y in points for action='drag'." },
                "text": { "type": "string", "description": "Text to type for action='type'." },
                "keys": {
                    "type": "string",
                    "description": "Key chord for action='key', e.g. 'cmd+space', 'return', \
                        'esc', 'ctrl+shift+t', 'cmd+,'. Modifiers: cmd, ctrl/control, alt/option, shift, fn."
                },
                "dx": { "type": "integer", "description": "Horizontal scroll amount for action='scroll'." },
                "dy": { "type": "integer", "description": "Vertical scroll amount for action='scroll' (positive scrolls up)." },
                "depth": { "type": "integer", "description": "Max Accessibility tree depth for action='ui' (default 12)." }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let parsed: ComputerInput =
            serde_json::from_value(input).context("invalid `computer` tool input")?;
        // The heavy lifting (synthetic events, screencapture, osascript) is
        // blocking, so run it off the async runtime.
        tokio::task::spawn_blocking(move || run(parsed))
            .await
            .context("computer tool task panicked")?
    }
}

#[cfg(not(target_os = "macos"))]
fn run(_input: ComputerInput) -> Result<ToolOutput> {
    bail!("The `computer` tool is only supported on macOS.")
}

#[cfg(target_os = "macos")]
fn run(input: ComputerInput) -> Result<ToolOutput> {
    match input.action.as_str() {
        "screenshot" => macos::screenshot(),
        "check_permissions" => macos::check_permissions(),
        "cursor" => macos::cursor_position(),
        "move" => {
            let (x, y) = require_xy(&input)?;
            macos::move_to(x, y)
        }
        "click" => macos::click(input.x, input.y, macos::Button::Left, 1),
        "double_click" => macos::click(input.x, input.y, macos::Button::Left, 2),
        "right_click" => macos::click(input.x, input.y, macos::Button::Right, 1),
        "drag" => {
            let (x, y) = require_xy(&input)?;
            match (input.to_x, input.to_y) {
                (Some(tx), Some(ty)) => macos::drag(x, y, tx, ty),
                _ => bail!("action='drag' requires `to_x` and `to_y`"),
            }
        }
        "type" => {
            let text = input
                .text
                .as_deref()
                .filter(|s| !s.is_empty())
                .context("action='type' requires a non-empty `text`")?;
            macos::type_text(text)
        }
        "key" => {
            let keys = input
                .keys
                .as_deref()
                .filter(|s| !s.is_empty())
                .context("action='key' requires a `keys` chord, e.g. 'cmd+space'")?;
            macos::key_chord(keys)
        }
        "scroll" => {
            let dx = input.dx.unwrap_or(0);
            let dy = input.dy.unwrap_or(0);
            if dx == 0 && dy == 0 {
                bail!("action='scroll' requires a non-zero `dx` and/or `dy`");
            }
            macos::scroll(input.x, input.y, dx, dy)
        }
        "ui" => macos::ui_tree(input.depth.unwrap_or(12)),
        other => bail!(
            "Unknown computer action: {other}. Valid: screenshot, move, click, \
             double_click, right_click, drag, type, key, scroll, ui, cursor, check_permissions"
        ),
    }
}

#[cfg(target_os = "macos")]
fn require_xy(input: &ComputerInput) -> Result<(f64, f64)> {
    match (input.x, input.y) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => bail!("action='{}' requires both `x` and `y`", input.action),
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use core_graphics::display::CGDisplay;
    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventType, CGMouseButton, ScrollEventUnit,
    };
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;
    use std::process::Command;
    use std::thread::sleep;
    use std::time::Duration;

    #[derive(Clone, Copy)]
    pub enum Button {
        Left,
        Right,
    }

    fn source() -> Result<CGEventSource> {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow::anyhow!("failed to create CGEventSource (Accessibility permission?)"))
    }

    fn post(event: CGEvent) {
        use core_graphics::event::CGEventTapLocation;
        event.post(CGEventTapLocation::HID);
    }

    /// Current cursor position in global (top-left origin) screen points.
    fn current_cursor() -> Result<CGPoint> {
        // An event created with no specific position reports the current cursor
        // location via `location()`.
        let src = source()?;
        let evt = CGEvent::new(src).map_err(|_| anyhow::anyhow!("failed to read cursor position"))?;
        Ok(evt.location())
    }

    pub fn cursor_position() -> Result<ToolOutput> {
        let p = current_cursor()?;
        Ok(ToolOutput::new(format!("cursor at ({:.0}, {:.0})", p.x, p.y))
            .with_metadata(json!({ "x": p.x, "y": p.y })))
    }

    pub fn move_to(x: f64, y: f64) -> Result<ToolOutput> {
        let src = source()?;
        let evt = CGEvent::new_mouse_event(
            src,
            CGEventType::MouseMoved,
            CGPoint::new(x, y),
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow::anyhow!("failed to create mouse-move event"))?;
        post(evt);
        Ok(ToolOutput::new(format!("moved cursor to ({x:.0}, {y:.0})")))
    }

    pub fn click(x: Option<f64>, y: Option<f64>, button: Button, count: u32) -> Result<ToolOutput> {
        let point = match (x, y) {
            (Some(x), Some(y)) => CGPoint::new(x, y),
            _ => current_cursor()?,
        };
        let (down, up, cg_button) = match button {
            Button::Left => (
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGMouseButton::Left,
            ),
            Button::Right => (
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGMouseButton::Right,
            ),
        };

        // Move first so the target app sees the cursor under the click.
        let src = source()?;
        let mv = CGEvent::new_mouse_event(src, CGEventType::MouseMoved, point, cg_button)
            .map_err(|_| anyhow::anyhow!("failed to create move event"))?;
        post(mv);
        sleep(Duration::from_millis(10));

        for i in 1..=count {
            let src_d = source()?;
            let down_evt = CGEvent::new_mouse_event(src_d, down, point, cg_button)
                .map_err(|_| anyhow::anyhow!("failed to create mouse-down event"))?;
            // For multi-click (double click) set the click state field so apps
            // recognize it as a double click rather than two singles.
            if count > 1 {
                use core_graphics::event::EventField;
                down_evt.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i as i64);
            }
            post(down_evt);

            let src_u = source()?;
            let up_evt = CGEvent::new_mouse_event(src_u, up, point, cg_button)
                .map_err(|_| anyhow::anyhow!("failed to create mouse-up event"))?;
            if count > 1 {
                use core_graphics::event::EventField;
                up_evt.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i as i64);
            }
            post(up_evt);
            sleep(Duration::from_millis(20));
        }

        let label = match button {
            Button::Left if count >= 2 => "double-clicked",
            Button::Left => "clicked",
            Button::Right => "right-clicked",
        };
        Ok(ToolOutput::new(format!(
            "{label} at ({:.0}, {:.0})",
            point.x, point.y
        )))
    }

    pub fn drag(from_x: f64, from_y: f64, to_x: f64, to_y: f64) -> Result<ToolOutput> {
        let from = CGPoint::new(from_x, from_y);
        let to = CGPoint::new(to_x, to_y);

        // Press at the start.
        let src = source()?;
        let down = CGEvent::new_mouse_event(src, CGEventType::LeftMouseDown, from, CGMouseButton::Left)
            .map_err(|_| anyhow::anyhow!("failed to create drag-down event"))?;
        post(down);
        sleep(Duration::from_millis(30));

        // Move in a few steps so apps that track drag deltas follow along.
        let steps = 10;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let p = CGPoint::new(from_x + (to_x - from_x) * t, from_y + (to_y - from_y) * t);
            let src_m = source()?;
            let mv =
                CGEvent::new_mouse_event(src_m, CGEventType::LeftMouseDragged, p, CGMouseButton::Left)
                    .map_err(|_| anyhow::anyhow!("failed to create drag-move event"))?;
            post(mv);
            sleep(Duration::from_millis(15));
        }

        // Release at the destination.
        let src_u = source()?;
        let up = CGEvent::new_mouse_event(src_u, CGEventType::LeftMouseUp, to, CGMouseButton::Left)
            .map_err(|_| anyhow::anyhow!("failed to create drag-up event"))?;
        post(up);

        Ok(ToolOutput::new(format!(
            "dragged from ({from_x:.0}, {from_y:.0}) to ({to_x:.0}, {to_y:.0})"
        )))
    }

    /// Report whether the AX (Accessibility) and Screen Recording permissions
    /// appear to be granted, with guidance when they are not.
    pub fn check_permissions() -> Result<ToolOutput> {
        // Accessibility: try reading the frontmost process via System Events.
        // A permission failure surfaces as a specific osascript error.
        let ax = Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg("tell application \"System Events\" to get name of first application process whose frontmost is true")
            .output();
        let ax_ok = matches!(ax, Ok(ref o) if o.status.success());

        // Screen Recording: a successful, non-empty capture implies access.
        let tmp = std::env::temp_dir().join(format!("jcode_perm_{}.png", std::process::id()));
        let shot = Command::new("/usr/sbin/screencapture")
            .arg("-x")
            .arg(&tmp)
            .status();
        let screen_ok = matches!(shot, Ok(s) if s.success())
            && std::fs::metadata(&tmp).map(|m| m.len() > 0).unwrap_or(false);
        let _ = std::fs::remove_file(&tmp);

        let mut lines = vec![
            format!("Accessibility (input + UI tree): {}", yes_no(ax_ok)),
            format!("Screen Recording (screenshot):   {}", yes_no(screen_ok)),
        ];
        if !ax_ok {
            lines.push(
                "Grant Accessibility: System Settings > Privacy & Security > Accessibility \
                 (add your terminal / jcode and toggle it on)."
                    .to_string(),
            );
        }
        if !screen_ok {
            lines.push(
                "Grant Screen Recording: System Settings > Privacy & Security > Screen Recording \
                 (add your terminal / jcode and toggle it on)."
                    .to_string(),
            );
        }
        Ok(ToolOutput::new(lines.join("\n")).with_metadata(json!({
            "accessibility": ax_ok,
            "screen_recording": screen_ok,
        })))
    }

    fn yes_no(b: bool) -> &'static str {
        if b {
            "granted"
        } else {
            "NOT granted"
        }
    }

    pub fn type_text(text: &str) -> Result<ToolOutput> {
        // Send the whole string as a single synthesized keyboard event using the
        // Unicode string payload. This avoids per-character keycode mapping and
        // works for any layout / non-ASCII text.
        let src = source()?;
        let down = CGEvent::new_keyboard_event(src, 0, true)
            .map_err(|_| anyhow::anyhow!("failed to create keyboard event"))?;
        down.set_string(text);
        post(down);

        let src_up = source()?;
        let up = CGEvent::new_keyboard_event(src_up, 0, false)
            .map_err(|_| anyhow::anyhow!("failed to create keyboard event"))?;
        up.set_string(text);
        post(up);

        Ok(ToolOutput::new(format!("typed {} characters", text.chars().count())))
    }

    pub fn key_chord(chord: &str) -> Result<ToolOutput> {
        let mut flags = CGEventFlags::CGEventFlagNull;
        let mut keycode: Option<u16> = None;

        for raw in chord.split('+') {
            let part = raw.trim().to_lowercase();
            if part.is_empty() {
                continue;
            }
            match part.as_str() {
                "cmd" | "command" | "meta" | "super" => flags |= CGEventFlags::CGEventFlagCommand,
                "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
                "alt" | "opt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
                "shift" => flags |= CGEventFlags::CGEventFlagShift,
                "fn" => flags |= CGEventFlags::CGEventFlagSecondaryFn,
                other => {
                    if keycode.is_some() {
                        bail!("key chord '{chord}' has more than one non-modifier key");
                    }
                    keycode = Some(
                        keycode_for(other)
                            .with_context(|| format!("unknown key '{other}' in chord '{chord}'"))?,
                    );
                }
            }
        }

        let code = keycode.with_context(|| format!("chord '{chord}' has no main key"))?;

        let src = source()?;
        let down = CGEvent::new_keyboard_event(src, code, true)
            .map_err(|_| anyhow::anyhow!("failed to create key-down event"))?;
        down.set_flags(flags);
        post(down);
        sleep(Duration::from_millis(15));

        let src_up = source()?;
        let up = CGEvent::new_keyboard_event(src_up, code, false)
            .map_err(|_| anyhow::anyhow!("failed to create key-up event"))?;
        up.set_flags(flags);
        post(up);

        Ok(ToolOutput::new(format!("pressed {chord}")))
    }

    pub fn scroll(x: Option<f64>, y: Option<f64>, dx: i32, dy: i32) -> Result<ToolOutput> {
        // Position the cursor over the scroll target first, if requested.
        if let (Some(x), Some(y)) = (x, y) {
            move_to(x, y)?;
            sleep(Duration::from_millis(10));
        }
        let src = source()?;
        // wheel1 = vertical, wheel2 = horizontal.
        let evt = CGEvent::new_scroll_event(src, ScrollEventUnit::PIXEL, 2, dy, dx, 0)
            .map_err(|_| anyhow::anyhow!("failed to create scroll event"))?;
        post(evt);
        Ok(ToolOutput::new(format!("scrolled dx={dx} dy={dy}")))
    }

    pub fn screenshot() -> Result<ToolOutput> {
        let tmp = std::env::temp_dir().join(format!("jcode_computer_{}.png", std::process::id()));
        let status = Command::new("/usr/sbin/screencapture")
            .arg("-x") // no sound
            .arg(&tmp)
            .status()
            .context("failed to run screencapture")?;
        if !status.success() {
            bail!("screencapture failed (exit {:?})", status.code());
        }
        let bytes = std::fs::read(&tmp).context("failed to read screenshot file")?;
        let _ = std::fs::remove_file(&tmp);

        if bytes.is_empty() {
            bail!(
                "screenshot was empty. Grant Screen Recording permission to your \
                 terminal/jcode in System Settings > Privacy & Security > Screen Recording."
            );
        }

        let display = CGDisplay::main();
        let bounds = display.bounds();
        let pixel_w = display.pixels_wide();
        let pixel_h = display.pixels_high();
        // Retina displays render N physical pixels per point. Clicks/moves use
        // points, but the screenshot image is in pixels, so the model must
        // divide image pixel coordinates by this scale to get click coordinates.
        let scale = if bounds.size.width > 0.0 {
            pixel_w as f64 / bounds.size.width
        } else {
            1.0
        };
        let summary = format!(
            "Captured main display: {pixel_w}x{pixel_h} pixels = {:.0}x{:.0} points (scale {scale:.2}x). \
             IMPORTANT: click/move coordinates are in POINTS. To click something you see in this \
             image at pixel (px, py), use x = px / {scale:.2}, y = py / {scale:.2}.",
            bounds.size.width, bounds.size.height
        );

        Ok(ToolOutput::new(summary)
            .with_title("screenshot")
            .with_labeled_image("image/png", STANDARD.encode(&bytes), "screen")
            .with_metadata(json!({
                "width_points": bounds.size.width,
                "height_points": bounds.size.height,
                "width_pixels": pixel_w,
                "height_pixels": pixel_h,
                "scale": scale,
            })))
    }

    pub fn ui_tree(depth: u32) -> Result<ToolOutput> {
        // Walk the Accessibility tree of the frontmost app via System Events.
        // We keep this conservative (titles, roles, positions) and bounded by
        // depth to avoid pathological dumps.
        let script = format!(
            r#"
on dumpEl(el, lvl, maxlvl)
    set out to ""
    if lvl > maxlvl then return out
    try
        set r to (role of el as text)
    on error
        set r to "?"
    end try
    set t to ""
    try
        set t to (title of el as text)
    end try
    if t is "" then
        try
            set t to (value of el as text)
        end try
    end if
    set d to ""
    try
        set d to (description of el as text)
    end try
    set pos to ""
    try
        set p to position of el
        set sz to size of el
        set pos to " @(" & (item 1 of p) & "," & (item 2 of p) & " " & (item 1 of sz) & "x" & (item 2 of sz) & ")"
    end try
    set indent to ""
    repeat lvl times
        set indent to indent & "  "
    end repeat
    set line to indent & r
    if t is not "" then set line to line & " \"" & t & "\""
    if d is not "" then set line to line & " [" & d & "]"
    set line to line & pos & linefeed
    set out to out & line
    try
        repeat with child in (UI elements of el)
            set out to out & dumpEl(child, lvl + 1, maxlvl)
        end repeat
    end try
    return out
end dumpEl

tell application "System Events"
    set frontApp to first application process whose frontmost is true
    set appName to name of frontApp
    set out to "Frontmost app: " & appName & linefeed
    try
        set win to front window of frontApp
        set out to out & my dumpEl(win, 0, {depth})
    on error errMsg
        set out to out & "(no window / " & errMsg & ")"
    end try
    return out
end tell
"#,
            depth = depth
        );

        let output = Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .context("failed to run osascript for UI tree")?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            if err.contains("assistive") || err.contains("not allowed") || err.contains("1002") {
                bail!(
                    "Accessibility permission required. Grant it in System Settings > \
                     Privacy & Security > Accessibility for your terminal/jcode. ({})",
                    err.trim()
                );
            }
            bail!("UI tree read failed: {}", err.trim());
        }

        let tree = String::from_utf8_lossy(&output.stdout).trim_end().to_string();
        let tree = if tree.is_empty() {
            "(empty Accessibility tree)".to_string()
        } else {
            tree
        };
        Ok(ToolOutput::new(tree).with_title("ui tree"))
    }

    /// Map a key name to a layout-independent CGKeyCode (US virtual keycodes).
    fn keycode_for(key: &str) -> Option<u16> {
        use core_graphics::event::KeyCode;
        let code = match key {
            // Named keys via the crate's published constants.
            "return" | "enter" => KeyCode::RETURN,
            "tab" => KeyCode::TAB,
            "space" => KeyCode::SPACE,
            "delete" | "backspace" => KeyCode::DELETE,
            "esc" | "escape" => KeyCode::ESCAPE,
            "left" => KeyCode::LEFT_ARROW,
            "right" => KeyCode::RIGHT_ARROW,
            "down" => KeyCode::DOWN_ARROW,
            "up" => KeyCode::UP_ARROW,
            "home" => KeyCode::HOME,
            "end" => KeyCode::END,
            "pageup" => KeyCode::PAGE_UP,
            "pagedown" => KeyCode::PAGE_DOWN,
            "forwarddelete" => KeyCode::FORWARD_DELETE,
            // Letters and digits / punctuation via the ANSI keycode table below.
            other => return ansi_keycode(other),
        };
        Some(code)
    }

    /// US ANSI virtual keycodes for letters, digits, and common punctuation.
    /// These are layout-independent hardware positions.
    fn ansi_keycode(key: &str) -> Option<u16> {
        let mut chars = key.chars();
        let first = chars.next()?;
        if chars.next().is_some() {
            // Multi-char unknown key.
            return None;
        }
        let code: u16 = match first {
            'a' => 0x00,
            'b' => 0x0B,
            'c' => 0x08,
            'd' => 0x02,
            'e' => 0x0E,
            'f' => 0x03,
            'g' => 0x05,
            'h' => 0x04,
            'i' => 0x22,
            'j' => 0x26,
            'k' => 0x28,
            'l' => 0x25,
            'm' => 0x2E,
            'n' => 0x2D,
            'o' => 0x1F,
            'p' => 0x23,
            'q' => 0x0C,
            'r' => 0x0F,
            's' => 0x01,
            't' => 0x11,
            'u' => 0x20,
            'v' => 0x09,
            'w' => 0x0D,
            'x' => 0x07,
            'y' => 0x10,
            'z' => 0x06,
            '0' => 0x1D,
            '1' => 0x12,
            '2' => 0x13,
            '3' => 0x14,
            '4' => 0x15,
            '5' => 0x17,
            '6' => 0x16,
            '7' => 0x1A,
            '8' => 0x1C,
            '9' => 0x19,
            '-' => 0x1B,
            '=' => 0x18,
            '[' => 0x21,
            ']' => 0x1E,
            '\\' => 0x2A,
            ';' => 0x29,
            '\'' => 0x27,
            ',' => 0x2B,
            '.' => 0x2F,
            '/' => 0x2C,
            '`' => 0x32,
            _ => return None,
        };
        Some(code)
    }

    #[cfg(test)]
    mod tests {
        use super::{ansi_keycode, keycode_for};

        #[test]
        fn maps_named_keys() {
            assert_eq!(keycode_for("return"), Some(0x24));
            assert_eq!(keycode_for("space"), Some(0x31));
            assert_eq!(keycode_for("esc"), Some(0x35));
            assert_eq!(keycode_for("escape"), Some(0x35));
            assert_eq!(keycode_for("left"), Some(0x7B));
        }

        #[test]
        fn maps_letters_and_digits() {
            assert_eq!(keycode_for("a"), Some(0x00));
            assert_eq!(keycode_for("z"), Some(0x06));
            assert_eq!(keycode_for("0"), Some(0x1D));
            assert_eq!(keycode_for(","), Some(0x2B));
        }

        #[test]
        fn rejects_unknown_keys() {
            assert_eq!(ansi_keycode("nope"), None);
            assert_eq!(keycode_for("nope"), None);
        }
    }
}
