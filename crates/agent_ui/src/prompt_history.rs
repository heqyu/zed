//! User prompt history dropdown for the [`AgentPanel`].
//!
//! Shown as a popover triggered from a button in the agent panel's toolbar.
//! Lists the user prompts submitted in the current thread, with timestamp,
//! and lets the user click any entry to scroll the chat view to that message.
//!
//! Build stages (current: **Stage 1** — data capture):
//!   - Stage 0: ✅ skeleton — toggle button wired up, popover renders a header.
//!   - Stage 1: ✅ subscribe to `AcpThreadEvent::NewEntry`, capture
//!              `{ id, entry_index, preview, timestamp }` for each `UserMessage`;
//!              prune on `EntriesRemoved`.
//!   - Stage 2: render the captured list with `HH:MM:SS` timestamps.
//!   - Stage 3: click a row → `list_state.scroll_to_reveal_item(entry_index)`.
//!   - Stage 4: scroll-to-top / scroll-to-bottom buttons in the popover header.
//!   - Stage 5: tooltip with full content, esc-to-close.

use std::ops::Range;

use acp_thread::{AgentThreadEntry, ContentBlock, UserMessageId};
use chrono::{DateTime, Local};
use gpui::{App, Entity, ListOffset, ListState, Pixels, SharedString, Window};
use ui::{Color, ContextMenu, IconButton, IconName, IconSize, Label, LabelSize, Tooltip, prelude::*};

/// One captured user prompt.
///
/// `entry_index` is the position of this prompt inside the thread's
/// `Vec<AgentThreadEntry>` **at the moment the navigation happens**. It is
/// kept in sync via [`PromptHistory::handle_removal`] when the user rewinds
/// or otherwise removes a range of entries.
///
/// `id` is the (optional) stable identifier from the backend — preferred for
/// future re-resolution if `entry_index` ever drifts; not used in Stage 1.
#[derive(Debug, Clone)]
pub struct PromptHistoryEntry {
    /// Stable backend identity for the user message, when the agent provides one.
    /// Reserved for Stage 3 — used to re-resolve `entry_index` if the click
    /// handler ever needs to defend against drift not caught by `handle_removal`.
    #[allow(dead_code)]
    pub id: Option<UserMessageId>,
    /// Position inside the chat's `AgentThreadEntry` list. Maintained on remove.
    pub entry_index: usize,
    /// Snapshot of the prompt text, already truncated for the dropdown row.
    pub preview: SharedString,
    /// Full text of the prompt (Stage 5: shown as a tooltip on hover so the
    /// user can read past the row's truncation). Same placeholder rules as
    /// `preview` for non-text content blocks.
    pub full_text: SharedString,
    /// Captured panel-side at insertion (or bootstrap) time — `UserMessage`
    /// itself has no timestamp field.
    pub timestamp: DateTime<Local>,
}

/// In-memory list of prompts captured for a single thread.
///
/// Lifetime: owned by [`AgentPanel`], reset whenever the active thread changes.
#[derive(Debug, Default)]
pub struct PromptHistory {
    entries: Vec<PromptHistoryEntry>,
}

impl PromptHistory {
    pub fn entries(&self) -> &[PromptHistoryEntry] {
        &self.entries
    }

    /// Drop everything — called when the active thread changes.
    pub fn reset(&mut self) {
        self.entries.clear();
    }

    /// Bootstrap from a thread's existing entries (e.g. when the user reopens
    /// a previously-saved thread). Timestamps for these prompts use
    /// [`Local::now`], because the backend doesn't store the original send
    /// time. They will be visually marked as approximate in Stage 2.
    pub fn bootstrap_from_entries(
        &mut self,
        entries: &[AgentThreadEntry],
        preview_max_chars: usize,
        cx: &App,
    ) {
        self.entries.clear();
        for (idx, entry) in entries.iter().enumerate() {
            if let AgentThreadEntry::UserMessage(msg) = entry {
                let (full_text, preview) =
                    full_and_preview_from_content(&msg.content, preview_max_chars, cx);
                self.entries.push(PromptHistoryEntry {
                    id: msg.id.clone(),
                    entry_index: idx,
                    preview,
                    full_text,
                    timestamp: Local::now(),
                });
            }
        }
    }

    /// Called from the `AcpThreadEvent::NewEntry` handler. The event carries no
    /// payload, but `push_entry` always emits *after* appending, so the new
    /// entry is the last one in the slice — see `acp_thread.rs:1865`.
    pub fn capture_if_user_message(
        &mut self,
        entries: &[AgentThreadEntry],
        preview_max_chars: usize,
        cx: &App,
    ) {
        let Some(idx) = entries.len().checked_sub(1) else {
            return;
        };
        let AgentThreadEntry::UserMessage(msg) = &entries[idx] else {
            return;
        };
        let (full_text, preview) =
            full_and_preview_from_content(&msg.content, preview_max_chars, cx);
        self.entries.push(PromptHistoryEntry {
            id: msg.id.clone(),
            entry_index: idx,
            preview,
            full_text,
            timestamp: Local::now(),
        });
    }

    /// Adjust history when entries are removed from the underlying thread
    /// (rewind, edit-and-resend, etc. — see `AcpThreadEvent::EntriesRemoved`).
    /// Drops captured entries inside the removed range and shifts the indices
    /// of any captured entries that came after.
    pub fn handle_removal(&mut self, removed: &Range<usize>) {
        let len = removed.end.saturating_sub(removed.start);
        self.entries.retain_mut(|e| {
            if removed.contains(&e.entry_index) {
                false
            } else {
                if e.entry_index >= removed.end {
                    e.entry_index -= len;
                }
                true
            }
        });
    }
}

/// Extract `(full_text, preview)` from a [`ContentBlock`].
///
/// `full_text` keeps the raw source (for Stage-5 hover tooltip), `preview` is
/// single-line + truncated to `preview_max_chars` for the dropdown row. Char
/// truncation alone isn't enough — CJK / emoji glyphs render much wider than
/// 1ch, so the popover layer also clamps row width via `max_w`. Non-`Markdown`
/// variants are rendered as bracketed placeholders so the row stays
/// informative without pulling in image / resource-link rendering — both
/// fields use the same placeholder so hover and row content match.
fn full_and_preview_from_content(
    content: &ContentBlock,
    preview_max_chars: usize,
    cx: &App,
) -> (SharedString, SharedString) {
    let raw: String = match content {
        ContentBlock::Empty => String::new(),
        ContentBlock::Markdown { markdown } => markdown.read(cx).source().to_string(),
        ContentBlock::ResourceLink { resource_link } => {
            format!("[file: {}]", resource_link.uri)
        }
        ContentBlock::Image { .. } => "[image]".to_string(),
    };
    let full_text: SharedString = raw.clone().into();
    let single_line = raw.lines().next().unwrap_or("").trim();
    let chars: Vec<char> = single_line.chars().collect();
    let preview: SharedString = if chars.len() > preview_max_chars {
        let mut s: String = chars.into_iter().take(preview_max_chars).collect();
        s.push('…');
        s.into()
    } else {
        single_line.to_string().into()
    };
    (full_text, preview)
}

/// Builds the popover content shown when the toolbar history button is clicked.
///
/// Stage 3: each row's click scrolls the chat to the corresponding entry via
/// the supplied `ListState`. When `list_state` is `None` (no active thread
/// view at popover construction time), click handlers are inert.
///
/// `ListState` is a thin `Rc<RefCell<…>>` newtype, so cloning it across rows
/// and into closures is cheap and keeps the same underlying scroll state.
///
/// `row_max_width` clamps the visual width of every row regardless of glyph
/// width — necessary because char-count truncation can't tell apart 1-ch
/// ASCII from 2-ch CJK / emoji rendering. Combined with `Label::truncate()`,
/// over-wide content collapses to an in-Label ellipsis instead of bursting
/// out of the panel.
pub fn build_prompt_history_menu(
    entries: &[PromptHistoryEntry],
    list_state: Option<ListState>,
    row_max_width: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ContextMenu> {
    // Snapshot for the move-into-closure pattern; ContextMenu's custom_entry
    // demands `'static` renderers, so we can't borrow from `entries`.
    let rows: Vec<PromptHistoryEntry> = entries.to_vec();
    let entry_count = rows.len();

    ContextMenu::build(window, cx, move |menu, _window, _cx| {
        let menu = menu.header(format!("Prompt History ({})", entry_count));
        if entry_count == 0 {
            return menu.label("No prompts in this session yet");
        }

        // Section separators give the popover three visually distinct bands —
        // header / scroll-controls / list — which is what the user feedback
        // ("叠在一起了") was actually after. ContextMenu's separator() uses
        // theme tokens (`border_variant`), so this works in both light and
        // dark themes without further tuning.
        let menu = menu.separator();

        // Stage 4: scroll-to-top / scroll-to-bottom row, rendered above the
        // list so the affordance is reachable without scrolling the popover
        // itself. Skipped when there is no list_state (e.g. no active thread
        // view at popover construction time) — the buttons would be inert
        // anyway, no need to clutter the UI.
        let menu = if let Some(state) = list_state.clone() {
            let top_state = state.clone();
            let bottom_state = state;
            menu.custom_row(move |_window, _cx| {
                let ts = top_state.clone();
                let bs = bottom_state.clone();
                h_flex()
                    .w_full()
                    .gap_1()
                    // Inline padding so the button row visually breathes
                    // against the surrounding separators.
                    .px_2()
                    .py_0p5()
                    .child(
                        IconButton::new("prompt-history-scroll-top", IconName::ArrowUp)
                            .icon_size(IconSize::Small)
                            .tooltip(|_window, cx| Tooltip::simple("Scroll chat to top", cx))
                            .on_click(move |_event, _window, _cx| {
                                ts.scroll_to(ListOffset::default());
                            }),
                    )
                    .child(
                        IconButton::new("prompt-history-scroll-bottom", IconName::ArrowDown)
                            .icon_size(IconSize::Small)
                            .tooltip(|_window, cx| {
                                Tooltip::simple("Scroll chat to bottom", cx)
                            })
                            .on_click(move |_event, _window, _cx| {
                                bs.scroll_to_end();
                            }),
                    )
                    .into_any_element()
            })
        } else {
            menu
        };

        // Separator between scroll-controls and the list itself.
        let menu = menu.separator();

        // Oldest first — matches the chat's vertical order, so "scroll to top"
        // (Stage 4) consistently means "first prompt of the conversation".
        let mut menu = menu;
        for entry in rows {
            let time = entry.timestamp.format("%H:%M:%S").to_string();
            let preview = entry.preview.clone();
            let full_text = entry.full_text.clone();
            let entry_index = entry.entry_index;
            let row_id = SharedString::from(format!("prompt-history-row-{}", entry_index));
            // Each row owns its own ListState clone; closures must be `'static`
            // so we can't borrow from the outer `list_state`.
            let row_list_state = list_state.clone();
            menu = menu.custom_entry(
                move |_window, _cx| {
                    let tooltip_text = full_text.clone();
                    h_flex()
                        .id(row_id.clone())
                        // Hard width clamp — independent of char count, so CJK
                        // / emoji rows can't burst the popover open.
                        .max_w(row_max_width)
                        .w_full()
                        .gap_2()
                        // Tight inner padding for a denser row rhythm than
                        // the default ContextMenuEntry leaves us with —
                        // helps the section feel its own visual block.
                        .px_2()
                        .py_0p5()
                        .overflow_hidden()
                        .child(
                            Label::new(time.clone())
                                .color(Color::Muted)
                                .size(LabelSize::Small),
                        )
                        // truncate() defers ellipsis to the layout layer, so
                        // it works even when `chars.len()` is small but the
                        // rendered width is still excessive.
                        .child(Label::new(preview.clone()).truncate())
                        // Stage 5: hover tooltip with the full prompt text so
                        // truncated rows are still recoverable. We deliberately
                        // pass the raw content unmodified — `Tooltip::simple`
                        // routes through plain-text rendering, so markdown /
                        // special characters are safe.
                        .tooltip(move |_window, cx| Tooltip::simple(tooltip_text.clone(), cx))
                        .into_any_element()
                },
                move |_window, _cx| {
                    if let Some(state) = row_list_state.as_ref() {
                        // Always pin the target row to the TOP of the chat
                        // viewport, irrespective of its current position
                        // relative to the scroll. `scroll_to_reveal_item`
                        // would minimise scroll distance instead — sending
                        // upward targets to the top but downward targets to
                        // the bottom, which felt asymmetric in user testing.
                        //
                        // Physical limit: if `entry_index` lies within the
                        // last viewport-height of the list, GPUI clamps the
                        // resulting scroll_top to scroll_max (list.rs:606),
                        // so the row ends up wherever the bottom of the list
                        // pins it. There's no way around that — the list
                        // simply doesn't have content past the end to pad
                        // the row up to the top with.
                        state.scroll_to(ListOffset {
                            item_ix: entry_index,
                            offset_in_item: gpui::Pixels::ZERO,
                        });
                    }
                },
            );
        }
        menu
    })
}
