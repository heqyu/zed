//! Session-level memory of the user's last view mode for each file.
//!
//! When a user toggles an editor between read-only and editable (or, later,
//! between markdown-rendered and source modes), we remember that decision so
//! reopening the same file in the same Zed session restores their choice
//! instead of snapping back to the [`editor.default_read_only_on_open`]
//! default.
//!
//! The memory is in-process only — restarting Zed wipes it. Per the design
//! discussion, a per-thread persistence-on-disk wasn't asked for.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, Global};
use util::rel_path::RelPath;

/// One file's last-known view mode.
///
/// Stage 2 only uses [`EditorViewMode::SourceReadOnly`] and
/// [`EditorViewMode::SourceEditable`]. [`EditorViewMode::MarkdownRendered`]
/// is reserved for stages 3-4 — kept here so the data shape doesn't churn
/// when the markdown swap lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorViewMode {
    /// Source code, editor-level read_only flag is on.
    SourceReadOnly,
    /// Source code, editor-level read_only flag is off.
    SourceEditable,
    /// Markdown rendered preview (used by markdown_preview crate).
    /// Reserved for stages 3-4. Treated by stage 2 as "no source-mode
    /// override" — the file simply opens in its default source-mode state.
    #[allow(dead_code)]
    MarkdownRendered,
}

impl EditorViewMode {
    /// Whether this mode corresponds to a source view that should be
    /// read-only-locked at the editor level.
    pub fn is_source_read_only(self) -> bool {
        matches!(self, EditorViewMode::SourceReadOnly)
    }
}

/// Process-wide registry of the user's last view mode per file path.
///
/// Stored as a [`gpui::Global`] so any code path that opens a file (Editor
/// constructor, future markdown preview swap, etc.) can consult it without
/// plumbing it through every layer.
///
/// Keyed by the buffer's worktree-relative `RelPath` (Zed's canonical
/// project-relative path type), since that is what `Buffer::file().path()`
/// returns. Two files with the same relative path in different worktrees
/// will share an entry — fine for a per-session UX hint, not OK if we ever
/// need cross-worktree distinction (which would require keying by absolute
/// path or worktree id + RelPath).
#[derive(Default)]
pub struct EditorViewModeMemory {
    by_path: HashMap<Arc<RelPath>, EditorViewMode>,
}

impl Global for EditorViewModeMemory {}

impl EditorViewModeMemory {
    pub fn get(cx: &App, path: &Arc<RelPath>) -> Option<EditorViewMode> {
        cx.try_global::<Self>()
            .and_then(|m| m.by_path.get(path).copied())
    }

    pub fn set(cx: &mut App, path: Arc<RelPath>, mode: EditorViewMode) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
        }
        cx.global_mut::<Self>().by_path.insert(path, mode);
    }
}
