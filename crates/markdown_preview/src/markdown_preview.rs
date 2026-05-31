use std::sync::Arc;

use editor::{
    Editor, EditorSettings,
    editor_view_mode_memory::{EditorViewMode, EditorViewModeMemory},
};
use gpui::{App, AppContext as _, Entity, Task, Window, actions};
use language::LanguageRegistry;
use project::{Project, ProjectEntryId, ProjectItem as _, ProjectPath};
use settings::Settings as _;
use workspace::{Workspace, WorkspaceItemBuilder, item::ItemHandle};

pub mod markdown_preview_view;

pub use zed_actions::preview::markdown::{OpenPreview, OpenPreviewToTheSide};

actions!(
    markdown,
    [
        /// Scrolls up by one page in the markdown preview.
        #[action(deprecated_aliases = ["markdown::MovePageUp"])]
        ScrollPageUp,
        /// Scrolls down by one page in the markdown preview.
        #[action(deprecated_aliases = ["markdown::MovePageDown"])]
        ScrollPageDown,
        /// Scrolls up by approximately one visual line.
        ScrollUp,
        /// Scrolls down by approximately one visual line.
        ScrollDown,
        /// Scrolls up by one markdown element in the markdown preview
        ScrollUpByItem,
        /// Scrolls down by one markdown element in the markdown preview
        ScrollDownByItem,
        /// Scrolls to the top of the markdown preview.
        ScrollToTop,
        /// Scrolls to the bottom of the markdown preview.
        ScrollToBottom,
        /// Opens a following markdown preview that syncs with the editor.
        OpenFollowingPreview,
        /// Toggles the active markdown file's tab between rendered preview
        /// and an editable source Editor, in place — closes the current item
        /// and adds a fresh one of the other type at the same pane index.
        /// Used by stage 4 of the read-only-default feature.
        ToggleMarkdownSourceMode
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        markdown_preview_view::MarkdownPreviewView::register(workspace, window, cx);
    })
    .detach();

    // Stage 3: route .md file opens through MarkdownPreviewView when the
    // editor.default_read_only_on_open setting is on, instead of falling
    // through to the default Editor opener. Registered AFTER editor::init
    // (see crates/zed/src/main.rs), so this opener gets first dibs in
    // ProjectItemRegistry::open_path's iter().rev().
    workspace::register_default_view_for_path(cx, open_md_as_preview);
}

/// Stage-4 action handler: swap the active pane's item between
/// [`MarkdownPreviewView`] and an editable source [`Editor`] for the same
/// underlying buffer, in place.
///
/// Strategy:
///   1. Look at the active pane's active item.
///   2. If it's a `MarkdownPreviewView`, recover its source editor (which
///      already owns the Buffer + Project), build a fresh editable Editor
///      around the same buffer, and replace the preview tab in place via
///      `Pane::replace_item_at`. Update the session memory to
///      `SourceEditable`.
///   3. If it's an `Editor` whose language is Markdown, build a new
///      `MarkdownPreviewView` wrapping the editor and replace the editor
///      tab in place. Update the session memory to `MarkdownRendered`.
///   4. Otherwise (non-md editor, no item, etc.) — no-op.
///
/// Why `replace_item_at` and not `add_item` + `close_item_by_id`:
/// after stage 6, both view shapes report the same `project_entry_id`,
/// so `add_item` deduplicates the new item against the existing preview
/// (since it's the active one) and silently drops it. The subsequent
/// close then leaves the tab empty. The dedicated swap primitive
/// bypasses that path entirely.
pub fn toggle_markdown_source_mode(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    use editor::editor_view_mode_memory::EditorViewMode as Mode;
    use editor::editor_view_mode_memory::EditorViewModeMemory;

    let pane = workspace.active_pane().clone();
    let (active_item, active_index) = {
        let pane_ref = pane.read(cx);
        let Some(item) = pane_ref.active_item() else {
            return;
        };
        (item, pane_ref.active_item_index())
    };

    // Branch A: preview → source
    if let Some(preview) = active_item.downcast::<markdown_preview_view::MarkdownPreviewView>() {
        let Some(source_editor) = preview.read(cx).source_editor() else {
            return;
        };
        let Some(buffer) = source_editor
            .read(cx)
            .buffer()
            .read(cx)
            .as_singleton()
        else {
            return;
        };
        let project = workspace.project().clone();
        let path = buffer.read(cx).file().map(|f| f.path().clone());

        // Build a fresh Editor for the buffer. for_buffer's read-only default
        // would normally re-apply, but our memory write below ensures
        // subsequent reopens stay in source-editable mode. We force-clear
        // read_only here because the user explicitly asked for source editing.
        let new_editor = cx.new(|cx_editor| {
            let mut editor = Editor::for_buffer(buffer.clone(), Some(project), window, cx_editor);
            editor.set_read_only(false);
            editor
        });

        if let Some(path) = path {
            EditorViewModeMemory::set(cx, path, Mode::SourceEditable);
        }

        pane.update(cx, |pane, cx| {
            pane.replace_item_at(active_index, Box::new(new_editor), window, cx);
        });
        return;
    }

    // Branch B: md source → preview
    if let Some(editor) = active_item.downcast::<Editor>() {
        let is_markdown = editor
            .read(cx)
            .buffer()
            .read(cx)
            .as_singleton()
            .and_then(|b| b.read(cx).language().map(|l| l.name().to_string()))
            .map(|name| name == "Markdown")
            .unwrap_or(false);
        if !is_markdown {
            return;
        }

        let path = editor
            .read(cx)
            .buffer()
            .read(cx)
            .as_singleton()
            .and_then(|b| b.read(cx).file().map(|f| f.path().clone()));

        let language_registry = workspace.project().read(cx).languages().clone();
        let workspace_handle = workspace.weak_handle();
        let preview = markdown_preview_view::MarkdownPreviewView::new(
            markdown_preview_view::MarkdownPreviewMode::Default,
            editor.clone(),
            workspace_handle,
            language_registry,
            window,
            cx,
        );

        if let Some(path) = path {
            EditorViewModeMemory::set(cx, path, Mode::MarkdownRendered);
        }

        pane.update(cx, |pane, cx| {
            pane.replace_item_at(active_index, Box::new(preview), window, cx);
        });
    }
}

/// Custom path opener that turns `.md` (and friends) into a
/// [`MarkdownPreviewView`] item. Returns `None` for any path that should
/// fall through to the default Editor opener.
fn open_md_as_preview(
    project: &Entity<Project>,
    project_path: &ProjectPath,
    window: &mut Window,
    cx: &mut App,
) -> Option<Task<anyhow::Result<(Option<ProjectEntryId>, WorkspaceItemBuilder)>>> {
    /// Markdown extensions that trigger preview-on-open. Lower-cased before
    /// comparison.
    const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mkd", "mdown"];

    // Filter 1: feature must be enabled.
    if !EditorSettings::get_global(cx).default_read_only_on_open {
        return None;
    }

    // Filter 2: file extension must be a markdown variant.
    let ext = project_path.path.extension().unwrap_or("");
    let ext_lower = ext.to_ascii_lowercase();
    if !MARKDOWN_EXTENSIONS.iter().any(|m| *m == ext_lower) {
        return None;
    }

    // Filter 3: if the user has already toggled this file to source mode in
    // the current session, respect that and let the default Editor opener
    // handle it. (Stage 4 keeps memory in sync when toggling.)
    if matches!(
        EditorViewModeMemory::get(cx, &project_path.path),
        Some(EditorViewMode::SourceEditable) | Some(EditorViewMode::SourceReadOnly)
    ) {
        return None;
    }

    // We're going to handle this path. Open the buffer, then defer the
    // actual view construction until we have a `Pane` (so we can grab a
    // `Workspace` handle from it for the Context-type swap that
    // `MarkdownPreviewView::new` requires).
    let buffer_task = project.update(cx, |project, cx| {
        project.open_buffer(project_path.clone(), cx)
    });
    let project_clone = project.clone();

    Some(window.spawn(cx, async move |cx| {
        let buffer = buffer_task.await?;
        // AsyncWindowContext::update takes (window, cx). Use it to peek into
        // the buffer once (synchronously, on the main thread) and grab the
        // entry id. `Buffer::entry_id` already returns `Option<...>`, so
        // `.ok().flatten()` collapses the Result into one Option.
        let project_entry_id = cx
            .update(|_window, cx| buffer.read(cx).entry_id(cx))
            .ok()
            .flatten();

        let build_workspace_item: WorkspaceItemBuilder = Box::new(
            move |pane: &mut workspace::Pane,
                  window: &mut Window,
                  cx_pane: &mut gpui::Context<workspace::Pane>| {
                let editor_for_preview = cx_pane.new(|cx| {
                    Editor::for_buffer(
                        buffer.clone(),
                        Some(project_clone.clone()),
                        window,
                        cx,
                    )
                });

                // MarkdownPreviewView::new requires `Context<Workspace>`. Hop
                // through the workspace handle the pane already keeps.
                let workspace_handle = pane.workspace().clone();
                let language_registry: Arc<LanguageRegistry> =
                    project_clone.read(cx_pane).languages().clone();

                let preview = workspace_handle
                    .update(cx_pane, |_workspace, cx| {
                        markdown_preview_view::MarkdownPreviewView::new(
                            markdown_preview_view::MarkdownPreviewMode::Default,
                            editor_for_preview,
                            workspace_handle.clone(),
                            language_registry,
                            window,
                            cx,
                        )
                    })
                    .expect(
                        "workspace was dropped while building the markdown preview item",
                    );

                Box::new(preview) as Box<dyn ItemHandle>
            },
        );

        Ok((project_entry_id, build_workspace_item))
    }))
}
