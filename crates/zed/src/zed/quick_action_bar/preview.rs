use csv_preview::{
    CsvPreviewView, OpenPreview as CsvOpenPreview, OpenPreviewToTheSide as CsvOpenPreviewToTheSide,
    TabularDataPreviewFeatureFlag,
};
use editor::EditorSettings;
use feature_flags::FeatureFlagAppExt as _;
use gpui::{AnyElement, Modifiers, WeakEntity};
use markdown_preview::{
    OpenPreview as MarkdownOpenPreview, OpenPreviewToTheSide as MarkdownOpenPreviewToTheSide,
    markdown_preview_view::MarkdownPreviewView,
};
use settings::Settings as _;
use svg_preview::{
    OpenPreview as SvgOpenPreview, OpenPreviewToTheSide as SvgOpenPreviewToTheSide,
    svg_preview_view::SvgPreviewView,
};
use ui::{Tooltip, prelude::*, text_for_keystroke};
use workspace::Workspace;

use super::QuickActionBar;

#[derive(Clone, Copy)]
enum PreviewType {
    Markdown,
    Svg,
    Csv,
}

impl QuickActionBar {
    pub fn render_preview_button(
        &self,
        workspace_handle: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut preview_type = None;

        // When default_read_only_on_open is on, opening a .md file already
        // produces a MarkdownPreviewView, and the toolbar's read-only
        // toggle button (👁) handles preview ↔ source swap. The legacy
        // "Preview Markdown" eye button (which opens an additional
        // preview pane) is redundant and visually indistinct from the
        // toggle, so we suppress just the Markdown branch in that mode.
        // SVG and CSV previews are untouched — they have no toggle
        // equivalent yet.
        let suppress_markdown = EditorSettings::get_global(cx).default_read_only_on_open;

        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                if !suppress_markdown
                    && MarkdownPreviewView::resolve_active_item_as_markdown_editor(workspace, cx)
                        .is_some()
                {
                    preview_type = Some(PreviewType::Markdown);
                } else if SvgPreviewView::resolve_active_item_as_svg_buffer(workspace, cx).is_some()
                {
                    preview_type = Some(PreviewType::Svg);
                } else if cx.has_flag::<TabularDataPreviewFeatureFlag>()
                    && CsvPreviewView::resolve_active_item_as_csv_editor(workspace, cx).is_some()
                {
                    preview_type = Some(PreviewType::Csv);
                }
            });
        }

        let preview_type = preview_type?;

        let (button_id, tooltip_text, open_action, open_to_side_action, open_action_for_tooltip) =
            match preview_type {
                PreviewType::Markdown => (
                    "toggle-markdown-preview",
                    "Preview Markdown",
                    Box::new(MarkdownOpenPreview) as Box<dyn gpui::Action>,
                    Box::new(MarkdownOpenPreviewToTheSide) as Box<dyn gpui::Action>,
                    &markdown_preview::OpenPreview as &dyn gpui::Action,
                ),
                PreviewType::Svg => (
                    "toggle-svg-preview",
                    "Preview SVG",
                    Box::new(SvgOpenPreview) as Box<dyn gpui::Action>,
                    Box::new(SvgOpenPreviewToTheSide) as Box<dyn gpui::Action>,
                    &svg_preview::OpenPreview as &dyn gpui::Action,
                ),
                PreviewType::Csv => (
                    "toggle-csv-preview",
                    "Preview CSV",
                    Box::new(CsvOpenPreview) as Box<dyn gpui::Action>,
                    Box::new(CsvOpenPreviewToTheSide) as Box<dyn gpui::Action>,
                    &csv_preview::OpenPreview as &dyn gpui::Action,
                ),
            };

        let alt_click = gpui::Keystroke {
            key: "click".into(),
            modifiers: Modifiers::alt(),
            ..Default::default()
        };

        let button = IconButton::new(button_id, IconName::Eye)
            .icon_size(IconSize::Small)
            .style(ButtonStyle::Subtle)
            .tooltip(move |_window, cx| {
                Tooltip::with_meta(
                    tooltip_text,
                    Some(open_action_for_tooltip),
                    format!(
                        "{} to open in a split",
                        text_for_keystroke(&alt_click.modifiers, &alt_click.key, cx)
                    ),
                    cx,
                )
            })
            .on_click(move |_, window, cx| {
                if let Some(workspace) = workspace_handle.upgrade() {
                    workspace.update(cx, |_, cx| {
                        if window.modifiers().alt {
                            window.dispatch_action(open_to_side_action.boxed_clone(), cx);
                        } else {
                            window.dispatch_action(open_action.boxed_clone(), cx);
                        }
                    });
                }
            });

        Some(button.into_any_element())
    }
}
