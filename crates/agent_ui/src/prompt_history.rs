//! User prompt history dropdown for the [`AgentPanel`].
//!
//! Shown as a popover triggered from a button in the agent panel's toolbar.
//! Lists the user prompts submitted in the current thread,
//! and lets the user click any entry to scroll the chat view to that message.

use std::ops::Range;

use acp_thread::{AgentThreadEntry, ContentBlock, UserMessageId};
use gpui::{App, Entity, ListOffset, ListState, Pixels, SharedString, Window};
use ui::{ContextMenu, IconButton, IconName, IconSize, Label, LabelSize, Tooltip, prelude::*};

/// One captured user prompt.
///
/// `entry_index` is the position of this prompt inside the thread's
/// `Vec<AgentThreadEntry>` **at the moment the navigation happens**. It is
/// kept in sync via [`PromptHistory::handle_removal`] when the user rewinds
/// or otherwise removes a range of entries.
#[derive(Debug, Clone)]
pub struct PromptHistoryEntry {
    /// Stable backend identity for the user message, when the agent provides one.
    #[allow(dead_code)]
    pub id: Option<UserMessageId>,
    /// Position inside the chat's `AgentThreadEntry` list. Maintained on remove.
    pub entry_index: usize,
    /// Snapshot of the prompt text, already truncated for the dropdown row.
    pub preview: SharedString,
    /// Full text of the prompt (shown as a tooltip on hover so the
    /// user can read past the row's truncation).
    pub full_text: SharedString,
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
    /// a previously-saved thread).
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
/// Each row's click scrolls the chat to the corresponding entry via the
/// supplied `ListState`. When `list_state` is `None` (no active thread view at
/// popover construction time), click handlers are inert.
pub fn build_prompt_history_menu(
    entries: &[PromptHistoryEntry],
    list_state: Option<ListState>,
    row_max_width: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ContextMenu> {
    let rows: Vec<PromptHistoryEntry> = entries.to_vec();
    let entry_count = rows.len();

    ContextMenu::build(window, cx, move |menu, _window, _cx| {
        if entry_count == 0 {
            return menu.label("No prompts in this session yet");
        }

        // Header row: title on the left, scroll buttons on the right.
        let menu = if let Some(state) = list_state.clone() {
            let top_state = state.clone();
            let bottom_state = state;
            menu.custom_row(move |_window, _cx| {
                let ts = top_state.clone();
                let bs = bottom_state.clone();
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .px_2()
                    .py_0p5()
                    .child(
                        Label::new(format!("Prompt History ({})", entry_count))
                            .size(LabelSize::Small),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                IconButton::new("prompt-history-scroll-top", IconName::ArrowUp)
                                    .icon_size(IconSize::Small)
                                    .tooltip(|_window, cx| {
                                        Tooltip::simple("Scroll chat to top", cx)
                                    })
                                    .on_click(move |_event, _window, _cx| {
                                        ts.scroll_to(ListOffset::default());
                                    }),
                            )
                            .child(
                                IconButton::new(
                                    "prompt-history-scroll-bottom",
                                    IconName::ArrowDown,
                                )
                                .icon_size(IconSize::Small)
                                .tooltip(|_window, cx| Tooltip::simple("Scroll chat to bottom", cx))
                                .on_click(
                                    move |_event, _window, _cx| {
                                        bs.scroll_to_end();
                                    },
                                ),
                            ),
                    )
                    .into_any_element()
            })
        } else {
            menu.header(format!("Prompt History ({})", entry_count))
        };

        let menu = menu.separator();

        // Oldest first — matches the chat's vertical order.
        let mut menu = menu;
        for entry in rows {
            let preview = entry.preview.clone();
            let full_text = entry.full_text.clone();
            let entry_index = entry.entry_index;
            let row_id = SharedString::from(format!("prompt-history-row-{}", entry_index));
            let row_list_state = list_state.clone();
            menu = menu.custom_entry(
                move |_window, _cx| {
                    let tooltip_text = full_text.clone();
                    h_flex()
                        .id(row_id.clone())
                        .max_w(row_max_width)
                        .w_full()
                        .px_2()
                        .py_0p5()
                        .overflow_hidden()
                        .child(Label::new(preview.clone()).truncate())
                        .tooltip(move |_window, cx| Tooltip::simple(tooltip_text.clone(), cx))
                        .into_any_element()
                },
                move |_window, _cx| {
                    if let Some(state) = row_list_state.as_ref() {
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
