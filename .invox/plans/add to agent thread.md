## Plan: Add "Add to Agent Thread" to Project Panel Context Menu

### Goal
Add an "Add to Agent Thread" (Ctrl-Shift-.) context menu item to the file explorer (project panel) right-click menu, which inserts the selected file as a mention into the active agent thread.

### Findings

- **Context menu built at** `crates/project_panel/src/project_panel.rs:1076` — the `deploy_context_menu` method constructs all context menu items using `ContextMenu::build`
- **Pattern for "Add to .gitignore"**: action defined in `crates/git/src/git.rs:110` via `actions!` macro, handler registered in `project_panel.rs:2269`, menu item at `project_panel.rs:1146`
- **`AddSelectionToThread` pattern** (cross-crate workspace action): defined at `crates/zed_actions/src/lib.rs:518`, workspace handler at `crates/agent_ui/src/agent_panel.rs:631` — gets `AgentPanel` from workspace, focuses it, calls `conversation_view.insert_selection()`
- **`ConversationView::insert_dragged_files`** at `crates/agent_ui/src/conversation_view.rs:2910` — takes `Vec<ProjectPath>`, calls `message_editor.insert_dragged_files()`
- **`MessageEditor::insert_dragged_files`** at `crates/agent_ui/src/message_editor.rs:1397` — calls `insert_mention_for_project_path` for each path with `MentionInsertPosition::EndOfBuffer`
- **`RelPath::to_proto()`** at `crates/util/src/rel_path.rs:215` — serializes to unix string; **`RelPath::from_proto()`** at line 220 — reconstructs from string
- **`WorktreeId::from_usize()`** at `crates/settings/src/settings.rs:95`; implements `Serialize` at line 85
- **`project_panel` does NOT depend on `agent_ui`** (`crates/project_panel/Cargo.toml`) — cross-panel communication must go through actions dispatched to the workspace
- **`ctrl-shift-.`** is already bound to `agent::AddSelectionToThread` in editor context (`assets/keymaps/default-windows.json:145`), but NOT in the project panel context — no conflict

### Proposed Changes

#### 1. `crates/zed_actions/src/lib.rs` — Add action (S)
Add a new `AddFileToThread` struct to the `agent` module, carrying the file path data:

```rust
/// Add a file as a mention to the active agent thread.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = agent)]
#[serde(deny_unknown_fields)]
pub struct AddFileToThread {
    pub worktree_id: usize,
    pub path: String,
}
```

Place it after the existing `PasteRaw` action (~line 522), alongside the other `agent` actions.

#### 2. `crates/agent_ui/src/agent_panel.rs` — Register workspace handler (M)
In the `init()` function (line ~349), inside the `cx.observe_new` block where other workspace actions are registered, add:

```rust
.register_action(
    |workspace: &mut Workspace, action: &AddFileToThread, window, cx| {
        let Some(agent_panel) = workspace.panel::<AgentPanel>(cx) else {
            return;
        };
        // Focus agent panel if not already focused
        if !agent_panel.focus_handle(cx).contains_focused(window, cx) {
            workspace.focus_panel::<AgentPanel>(window, cx);
        }
        // Reconstruct ProjectPath from action data
        let path = match RelPath::from_proto(&action.path) {
            Ok(p) => p,
            Err(_) => return,
        };
        let project_path = ProjectPath {
            worktree_id: WorktreeId::from_usize(action.worktree_id),
            path,
        };
        agent_panel.update(cx, |panel, cx| {
            cx.defer_in(window, move |panel, window, cx| {
                if let Some(conversation_view) = panel.active_conversation_view() {
                    conversation_view.update(cx, |cv, cx| {
                        cv.insert_dragged_files(
                            vec![project_path],
                            Vec::new(), // no new worktrees
                            window,
                            cx,
                        );
                    });
                }
            });
        });
    },
);
```

This follows the same pattern as `AddSelectionToThread` at line 631. Need to add imports for `RelPath`, `WorktreeId`, `ProjectPath`.

#### 3. `crates/project_panel/src/project_panel.rs` — Add context menu entry + handler (M)
In `deploy_context_menu()` (~line 1138), after the `.when(has_git_repo, ...)` block and before `.when(!should_hide_rename, ...)`, add:

```rust
.when(is_local, |menu| {
    menu.separator().item({
        let project_path_string = entry.path.to_proto();
        let wid = worktree_id.0;
        ContextMenuEntry::new("Add to Agent Thread")
            .key_binding(
                KeyBinding::for_action_in(
                    &agent::AddFileToThread { worktree_id: 0, path: String::new() },
                    &self.focus_handle,
                    cx,
                )
            )
            .handler(move |window, cx| {
                let action = agent::AddFileToThread {
                    worktree_id: wid,
                    path: project_path_string.clone(),
                };
                window.dispatch_action(action.boxed_clone(), cx);
            })
    })
})
```

Also register the action on the project panel's key handler (~line 6693):

```rust
.on_action(cx.listener(Self::add_file_to_thread))
```

And add the handler method:

```rust
fn add_file_to_thread(
    &mut self,
    action: &agent::AddFileToThread,
    window: &mut Window,
    cx: &mut Context<Self>,
) {
    // No-op here; action bubbles to workspace handler
}
```

Note: Import `zed_actions::agent` (already partially imported via `zed_actions`).

#### 4. `crates/project_panel/Cargo.toml` — No changes needed
`zed_actions` is already a dependency (line 48).

#### 5. `assets/keymaps/default-windows.json` — Add keybinding (S)
Add to the `"ProjectPanel"` or `"workspace"` key context section:

```json
"ctrl-shift-.": "agent::AddFileToThread"
```

This binds the shortcut when the project panel is focused. Similar entries should be added to `default-linux.json` and `default-macos.json` for cross-platform support.

### Risks

1. **`ctrl-shift-.` conflict**: Already used for `AddSelectionToThread` in the editor context. In the project panel context it's unbound, so no conflict. But if a user focuses the project panel and expects the editor shortcut, they might get the file-add behavior instead. Mitigation: use a different shortcut, or accept the context-specific behavior.

2. **`insert_dragged_files` is `pub(crate)`**: The workspace handler in `agent_panel.rs` calls `ConversationView::insert_dragged_files` which is `pub(crate)`. Both are in the `agent_ui` crate, so this works. If the workspace handler were moved elsewhere, it would break.

3. **Action serialization round-trip**: The `RelPath` is serialized as a unix-style string via `to_proto()` and reconstructed via `from_proto()`. If the path contains non-UTF-8 characters (unlikely but possible on some filesystems), reconstruction would fail silently.

### Open Questions

1. **Should the menu item appear for directories too?** `insert_mention_for_project_path` supports directories (`MentionUri::Directory`). The current plan shows it for all entries. Should it be restricted to files only?
2. **Should it be visible for remote projects?** The plan uses `.when(is_local, ...)`. Remote project support would require additional work.
3. **What happens if no agent thread is active?** The current plan follows the `AddSelectionToThread` pattern — if no active conversation view exists, the action silently does nothing. Should it create a new thread instead?

#### Answers
1. yes
2. 不考虑remote project
3. do nothing
