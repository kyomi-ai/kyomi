# Knowledge Page — Leptos Migration Plan

> Comprehensive, feature-complete migration of `/knowledge` from React to Leptos.
> Every feature in the React implementation must be present in the Leptos version —
> no shortcuts, no deferred features, no placeholders.

## Architecture Overview

### Source Files (React)
| File | Lines | Purpose |
|------|-------|---------|
| `pages/Knowledge.jsx` | 272 | Main page — layout, state orchestration, create/rename modal triggers |
| `components/KnowledgeFileTree.jsx` | 541 | Sidebar file tree — search, context menu, drag & drop |
| `components/KnowledgeFileEditor.jsx` | 283 | Editor pane — toolbar, auto-save, conflict detection, mode toggle |
| `components/CreateKnowledgeItemModal.jsx` | 81 | Create/rename dialog |
| `components/tiptap/TiptapDashboardEditor.jsx` | 963 | Visual WYSIWYG editor (Tiptap) |
| `components/MonacoMarkdownEditor.jsx` | 407 | Source code editor (Monaco) |
| **Total** | **2,547** | |

### Target Files (Leptos)
```
crates/kyomi-ui/src/
├── pages/
│   └── knowledge/
│       ├── mod.rs                        # Module exports
│       └── knowledge_page.rs             # Main page component
├── components/
│   └── knowledge/
│       ├── mod.rs                        # Module exports
│       ├── file_tree.rs                  # Sidebar file tree with search + context menu
│       ├── file_editor.rs                # Editor pane — toolbar, auto-save, conflict
│       ├── create_item_modal.rs          # Create/rename dialog
│       └── tree_types.rs                 # Shared types for tree operations
└── server_fns/
    └── knowledge.rs                      # Knowledge file server functions (new)
```

### Backend Services (Already Exist — No Changes Needed)
- `kyomi-knowledge/src/knowledge_files.rs` (1,189 lines) — CRUD, chunking, search, tree
- `kyomi-knowledge/src/models.rs` (91 lines) — Data models
- `kyomi-knowledge/src/vector_search.rs` — Semantic search
- `apps/server/src/routes/knowledge_files.rs` (554 lines) — REST endpoints

### Key Design Decision: Editor Strategy

The React knowledge page offers two editor modes:
1. **Visual mode** — Tiptap WYSIWYG (963 lines, JS-only library)
2. **Source mode** — Monaco editor (407 lines, JS-only library)

For Leptos, the approach follows the dashboard editor precedent:
- **Source mode** — Use `kode-leptos` (`CodeEditor` with `Language::Markdown`), already proven in `dashboard_editor.rs`
- **Visual mode** — Stub placeholder ("Visual editing coming soon"), matching dashboard editor's approach
- The MarkdownRenderer already exists in Leptos and provides a live preview of the markdown content

This avoids bringing in massive JS dependencies (Monaco ~4MB, Tiptap ~500KB) and matches the established pattern. When visual editing is needed later, it can be added as a separate phase.

---

## Phase 1: Knowledge Server Functions

> **Why first**: Every UI component needs data. Build the data layer so pages can
> fetch, create, update, and delete knowledge files.

### Task 1.1: Types + List/Get Server Functions
**File**: `crates/kyomi-ui/src/server_fns/knowledge.rs` (new)
**Estimated lines**: ~200

**Types**:
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeTreeEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub is_folder: bool,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeFileDetail {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub is_folder: bool,
    pub content: Option<String>,
    pub content_hash: Option<String>,
    pub sort_order: i32,
    pub created_by: Option<String>,     // Display name, not user_id
    pub updated_by: Option<String>,     // Display name
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeSearchResult {
    pub id: String,
    pub name: String,
    pub parent_path: Option<String>,    // "Folder / SubFolder"
    pub preview: String,                // First ~200 chars of content
    pub is_folder: bool,
}
```

**Server functions**:
```rust
#[server] pub async fn list_knowledge_tree() -> Result<Vec<KnowledgeTreeEntry>, ServerFnError>
#[server] pub async fn get_knowledge_file(file_id: String) -> Result<KnowledgeFileDetail, ServerFnError>
#[server] pub async fn search_knowledge_files(query: String) -> Result<Vec<KnowledgeSearchResult>, ServerFnError>
```

**Implementation**: Call `kyomi_knowledge::knowledge_files::*` directly. Use
`extract_auth()` and `extract_context()`. Resolve `created_by`/`updated_by` user IDs
to display names via user lookup (match the REST endpoint handler).

**Reference**: `apps/server/src/routes/knowledge_files.rs` handlers for exact service
calls and response mapping.

**Verification**: `cargo check --workspace`

### Task 1.2: Create/Update/Delete Server Functions
**File**: `crates/kyomi-ui/src/server_fns/knowledge.rs` (extend)
**Estimated lines**: ~200 (added)

**Server functions**:
```rust
#[server] pub async fn create_knowledge_file(
    name: String,
    parent_id: Option<String>,
    content: Option<String>,
    is_folder: bool,
) -> Result<KnowledgeFileDetail, ServerFnError>

#[server] pub async fn update_knowledge_file(
    file_id: String,
    content: Option<String>,
    content_hash: Option<String>,   // Required for CAS when updating content
    name: Option<String>,           // Rename
    parent_id: Option<Option<String>>,  // Move (None = don't move, Some(None) = move to root)
    sort_order: Option<i32>,        // Reorder
) -> Result<KnowledgeFileDetail, ServerFnError>

#[server] pub async fn delete_knowledge_file(file_id: String) -> Result<(), ServerFnError>
```

**Critical**: The `update_knowledge_file` function handles 4 distinct operations depending
on which fields are provided:
1. `content` + `content_hash` → content update with CAS (returns 409-equivalent error on conflict)
2. `name` → rename (enforce uniqueness per parent)
3. `parent_id` → move to different folder (with optional sort_order)
4. `sort_order` alone → reorder within current parent

The CAS conflict case must return a distinguishable error so the UI can show the
"Reload?" prompt instead of a generic error. Use a custom error variant or a specific
error message prefix like `"CONFLICT:"`.

**Reference**: `apps/server/src/routes/knowledge_files.rs` `update_file` handler,
especially the 409 conflict response.

**Verification**: `cargo check --workspace`

---

## Phase 2: Tree Types + Build Logic

> **Why second**: The file tree is built client-side from a flat list. This pure-logic
> module is needed by both the tree component and the main page.

### Task 2.1: Tree Building Logic
**File**: `crates/kyomi-ui/src/components/knowledge/tree_types.rs`
**Estimated lines**: ~120

**Types**:
```rust
#[derive(Clone, Debug)]
pub struct TreeNode {
    pub entry: KnowledgeTreeEntry,
    pub children: Vec<TreeNode>,
    pub depth: usize,
}
```

**Functions**:
```rust
/// Build a tree from a flat list of entries.
/// Sorts: folders before files, then alphabetically by name.
pub fn build_tree(entries: &[KnowledgeTreeEntry]) -> Vec<TreeNode>

/// Flatten a tree back to a depth-annotated list for rendering.
/// Returns (entry, depth, is_last_child) tuples.
pub fn flatten_tree(tree: &[TreeNode], expanded: &HashSet<String>) -> Vec<(KnowledgeTreeEntry, usize, bool)>

/// Get all descendant IDs of a node (for preventing circular drag-drop).
pub fn get_descendant_ids(entries: &[KnowledgeTreeEntry], node_id: &str) -> HashSet<String>

/// Build breadcrumb path string: "Folder / SubFolder / File.md"
pub fn build_path(entries: &[KnowledgeTreeEntry], file_id: &str) -> String

/// Get all folder entries (for "Move to" menu).
/// Excludes a node and its descendants.
pub fn get_folder_targets(entries: &[KnowledgeTreeEntry], exclude_id: &str) -> Vec<(String, String)>
```

**Reference**: React `KnowledgeFileTree.jsx` tree building logic (folders first, alphabetical).

**Verification**: `cargo check --workspace`

---

## Phase 3: Create/Rename Modal

> **Why third**: Small self-contained component used by both the tree and the page header.
> Build it early so it's ready to plug in.

### Task 3.1: Create/Rename Item Modal
**File**: `crates/kyomi-ui/src/components/knowledge/create_item_modal.rs`
**Estimated lines**: ~100

**Props**:
```rust
#[component]
pub fn CreateKnowledgeItemModal(
    #[prop(into)] show: Signal<bool>,
    #[prop(into)] on_close: Callback<()>,
    #[prop(into)] on_submit: Callback<String>,   // Called with trimmed name
    #[prop(into)] title: Signal<String>,          // "New File", "Rename", etc.
    #[prop(into)] default_value: Signal<String>,  // Pre-filled text
    #[prop(into)] submit_label: Signal<String>,   // "Create", "Rename", etc.
) -> impl IntoView
```

**Features**:
1. Uses existing `Modal` component from `crates/kyomi-ui/src/components/modal.rs`
2. Text input field with auto-focus + auto-select on open
3. Validation: name must not be empty after trimming
4. Submit on Enter key
5. Cancel button closes modal
6. Clears input on close

**Reference**: React `CreateKnowledgeItemModal.jsx` (81 lines).

**Verification**: `cargo check --workspace`

---

## Phase 4: File Tree Sidebar

> **Why fourth**: The tree is the primary navigation mechanism. Users select files here,
> create folders, search, and reorganize.

### Task 4.1: File Tree — Core Rendering + Selection
**File**: `crates/kyomi-ui/src/components/knowledge/file_tree.rs`
**Estimated lines**: ~300 (this task)

**Props**:
```rust
#[component]
pub fn KnowledgeFileTree(
    #[prop(into)] entries: Signal<Vec<KnowledgeTreeEntry>>,
    #[prop(into)] selected_id: Signal<Option<String>>,
    #[prop(into)] on_select: Callback<KnowledgeTreeEntry>,
    #[prop(into)] on_create_file: Callback<Option<String>>,    // parent_id
    #[prop(into)] on_create_folder: Callback<Option<String>>,  // parent_id
    #[prop(into)] on_rename: Callback<KnowledgeTreeEntry>,
    #[prop(into)] on_delete: Callback<String>,                 // file_id
    #[prop(into)] on_move: Callback<(String, Option<String>, i32)>,  // (file_id, new_parent_id, sort_order)
) -> impl IntoView
```

**Implement**:
1. **Tree building** from flat entries using `build_tree()` from Task 2.1
2. **Expand/collapse folders** — click folder row toggles, track in `RwSignal<HashSet<String>>`
3. **File/folder rendering**:
   - Indent based on depth (padding-left: `depth * 1rem`)
   - Icons: FolderOpen (expanded) / Folder (collapsed) / FileText (file)
   - Selected state: `bg-accent text-accent-foreground`
   - Click on file → `on_select` callback
   - Click on folder → toggle expand AND select
4. **Sort order**: Folders before files, then alphabetical within each group
5. **"New File" / "New Folder" buttons** in each folder row (visible on hover)
   - Pass folder's ID as `parent_id`
   - Root-level buttons in the sidebar header

**CSS**: Match React tree layout — sidebar width 288px (`w-72`), `border-r bg-card overflow-y-auto`.

**Reference**: React `KnowledgeFileTree.jsx` lines 1-200 (rendering).

**Verification**: `cargo check --workspace`

### Task 4.2: File Tree — Search
**File**: `crates/kyomi-ui/src/components/knowledge/file_tree.rs` (extend)
**Estimated lines**: ~120 (added)

**Implement**:
1. **Search input** at top of sidebar
   - 300ms debounce (same pattern as dashboards/chats list)
   - Clear button (X icon)
2. **Server-side search**: Call `search_knowledge_files(query)` server function
   - Show flat results with folder path breadcrumbs
   - Click result → select file, expand parent folders
3. **Fallback**: If server search fails, fall back to client-side name filter
   - ILIKE-style case-insensitive match on entry names
4. **Search results rendering**:
   - Flat list (not tree) while search is active
   - Show: icon + file name + parent path in muted text
   - Preview snippet if available
5. **Clear search** → return to normal tree view

**Reference**: React `KnowledgeFileTree.jsx` search logic.

**Verification**: `cargo check --workspace`, browser test: type in search, see results.

### Task 4.3: File Tree — Context Menu
**File**: `crates/kyomi-ui/src/components/knowledge/file_tree.rs` (extend)
**Estimated lines**: ~200 (added)

**Implement**:
1. **Right-click context menu** on any tree entry
   - Use a positioned `div` with `position: fixed` at mouse coordinates
   - Close on click outside or Escape key
   - Close on scroll
2. **Menu items**:
   - **Rename** → triggers `on_rename` callback (opens modal)
   - **Move to** → submenu listing all folders (except self and descendants)
     - Use `get_folder_targets()` from tree_types
     - "Root" option to move to top level
     - Click → calls `on_move(file_id, new_parent_id, sort_order)`
   - **Delete** → triggers `on_delete` callback (with confirm dialog)
3. **Separator line** between Rename and Move, Move and Delete
4. **Styling**: Match shadcn dropdown menu — `bg-popover text-popover-foreground border rounded-md shadow-md`

**Reference**: React `KnowledgeFileTree.jsx` context menu implementation.

**Verification**: `cargo check --workspace`

### Task 4.4: File Tree — Drag & Drop
**File**: `crates/kyomi-ui/src/components/knowledge/file_tree.rs` (extend)
**Estimated lines**: ~250 (added)

**Implement native HTML5 drag & drop** (React uses dnd-kit, but native DnD is simpler in Leptos):

1. **Drag start** — set `draggable="true"` on tree entries
   - Store dragged item ID in a signal
   - Add `opacity-40` class to dragged element
   - Set `effectAllowed = "move"` on DragEvent
2. **Drag over** — on folder targets:
   - Prevent default (to allow drop)
   - Highlight target: `bg-primary/10 ring-1 ring-primary/30`
   - Auto-expand folder after 500ms hover (if collapsed)
3. **Drop** — on folder targets:
   - Validate: can't drop on self or descendants (use `get_descendant_ids()`)
   - Call `on_move(dragged_id, target_folder_id, sort_order)`
   - Remove highlight
4. **Drag end** — clear dragged state, remove opacity
5. **Drag handle** — GripVertical icon, visible on hover (`opacity-0 group-hover:opacity-100`)
6. **Drop on root** — allow dropping on the sidebar background to move to root level

**CSS**: Match React drag states — `opacity-40` for dragging, `bg-primary/10 ring-1 ring-primary/30` for drop target.

**Reference**: React `KnowledgeFileTree.jsx` dnd-kit logic (adapted to native DnD).

**Verification**: `cargo check --workspace`, browser test: drag a file to a folder.

---

## Phase 5: File Editor

> **Why fifth**: The editor is the main content area. Source mode with live preview,
> auto-save with conflict detection, and toolbar.

### Task 5.1: Editor — Core Layout + File Loading
**File**: `crates/kyomi-ui/src/components/knowledge/file_editor.rs`
**Estimated lines**: ~250 (this task)

**Props**:
```rust
#[component]
pub fn KnowledgeFileEditor(
    #[prop(into)] selected_file: Signal<Option<KnowledgeTreeEntry>>,
    #[prop(into)] file_path: Signal<String>,              // Breadcrumb path
    #[prop(into)] on_saved: Callback<()>,                 // Refresh tree after save
) -> impl IntoView
```

**Implement**:
1. **Empty state** — when no file selected: "Select a file to edit" centered message
2. **Folder state** — when folder selected: "Select a file to view its contents"
3. **Loading state** — spinner + "Loading..." while fetching file content
4. **File loading effect** — when `selected_file` changes:
   - Cancel any pending auto-save
   - Fetch full file via `get_knowledge_file(file_id)` server function
   - Populate signals: `content`, `content_hash`, `updated_at`, `updated_by`
   - Guard against stale responses (if file changed while loading)
5. **Toolbar rendering**:
   - Left: file path breadcrumb
   - Center: save status indicator (Saving.../Saved/Conflict!)
   - Right: metadata "Updated [time] by [name]" (hidden on mobile)
   - Right: Source/Visual mode toggle buttons
6. **Editor area** — below toolbar, `flex-1 overflow-auto`
   - Source mode: `DashboardCodeEditor` (Kode) with `Language::Markdown`
   - Visual mode: stub "Visual editing coming soon" (matches dashboard editor)

**State signals**:
```rust
let (content, set_content) = signal(String::new());
let (content_hash, set_content_hash) = signal(Option::<String>::None);
let (save_status, set_save_status) = signal(SaveStatus::Idle);  // Idle | Saving | Saved | Conflict
let (updated_at, set_updated_at) = signal(Option::<String>::None);
let (updated_by, set_updated_by) = signal(Option::<String>::None);
let (mode, set_mode) = signal(EditorMode::Source);
let (is_loading, set_is_loading) = signal(false);
```

**Reference**: React `KnowledgeFileEditor.jsx` lines 1-150 (state + loading + toolbar).

**Verification**: `cargo check --workspace`, browser test: select a file, see content in editor.

### Task 5.2: Editor — Auto-Save with Conflict Detection
**File**: `crates/kyomi-ui/src/components/knowledge/file_editor.rs` (extend)
**Estimated lines**: ~200 (added)

**Implement**:
1. **Debounced auto-save** — 1500ms after last change
   - Use `gloo-timers::callback::Timeout` (same pattern as dashboard editor debounce)
   - Cancel pending save on new edit, file switch, or component unmount
   - Only trigger if content has actually changed from last saved version
2. **Save execution**:
   - Set `save_status = Saving`
   - Call `update_knowledge_file(file_id, content, content_hash)` server function
   - On success:
     - Update `content_hash` with new hash from response
     - Update `updated_at` and `updated_by`
     - Set `save_status = Saved`
     - Call `on_saved()` callback to refresh tree (name changes might affect tree)
   - On conflict (409 / "CONFLICT:" error):
     - Set `save_status = Conflict`
     - Show alert: "This file was modified by another user. Reload?" with action button
     - Make editor read-only until reload
   - On other error: show toast error, set `save_status = Idle`
3. **Reload handler** — fetch fresh content + hash, clear conflict state
4. **Stale request prevention** — track loaded file ID, ignore responses for previous files

**Reference**: React `KnowledgeFileEditor.jsx` lines 150-283 (auto-save + conflict).

**Verification**: `cargo check --workspace`, browser test: edit file, wait 1.5s, verify "Saved" appears.

### Task 5.3: Editor — Live Preview Panel
**File**: `crates/kyomi-ui/src/components/knowledge/file_editor.rs` (extend)
**Estimated lines**: ~80 (added)

**Implement**:
1. **Two-panel layout** (matching dashboard editor):
   - Left: Kode CodeEditor
   - Right: debounced MarkdownRenderer preview
   - Split 50/50 with divider
2. **Debounced preview** — update preview 600ms after edit stops
   - Uses same `gloo-timers` pattern as dashboard editor
3. **ChartML rendering** — MarkdownRenderer already handles ChartML blocks
4. **Scroll sync** (optional, can defer) — scroll preview when editor scrolls

**Reference**: `dashboard_editor.rs` two-panel layout (lines 610-660).

**Verification**: `cargo check --workspace`, browser test: edit markdown, see live preview update.

---

## Phase 6: Knowledge Page — Main Layout + Orchestration

> **Why sixth**: Wire the tree and editor together with the page header and modal logic.

### Task 6.1: Knowledge Page — Layout + State Orchestration
**File**: `crates/kyomi-ui/src/pages/knowledge/knowledge_page.rs`
**Estimated lines**: ~350

**Implement**:
1. **Page layout** — match React exactly:
   ```
   ┌────────────────────────────────────────────────┐
   │ Header: "Knowledge" + New File + New Folder    │
   ├──────────────┬─────────────────────────────────┤
   │ File Tree    │ File Editor                     │
   │ (w-72)       │ (flex-1)                        │
   │              │                                  │
   └──────────────┴─────────────────────────────────┘
   ```
   - CSS: `h-full flex flex-col bg-muted`
   - Header: `h-16 border-b bg-card` with title + action buttons
   - Content: `flex-1 overflow-hidden flex`
   - Sidebar: `w-72 border-r bg-card overflow-y-auto`
   - Editor: `flex-1 flex flex-col overflow-hidden`

2. **State management**:
   ```rust
   let (tree_entries, set_tree_entries) = signal(Vec::<KnowledgeTreeEntry>::new());
   let (selected_file, set_selected_file) = signal(Option::<KnowledgeTreeEntry>::None);
   let (selected_file_path, set_selected_file_path) = signal(String::new());
   let (modal_state, set_modal_state) = signal(ModalState::Hidden);
   ```

3. **Initial data load** — fetch tree via `list_knowledge_tree()` server function
   - Suspense with Spinner fallback

4. **File selection handler**:
   - Set `selected_file`
   - Compute `selected_file_path` via `build_path()` from tree_types

5. **Header buttons**:
   - "New File" → opens modal with `title="New File"`, `submit_label="Create"`, `parent_id=None`
   - "New Folder" → opens modal with `title="New Folder"`, `submit_label="Create"`, `parent_id=None`

6. **Wire components**:
   - `KnowledgeFileTree` with all callbacks
   - `KnowledgeFileEditor` with selected file + path signals
   - `CreateKnowledgeItemModal` controlled by `modal_state`

**Reference**: React `Knowledge.jsx` (272 lines) for exact layout and state orchestration.

**Verification**: `cargo check --workspace`, browser test: see page with sidebar and editor.

### Task 6.2: Knowledge Page — CRUD Operations
**File**: `crates/kyomi-ui/src/pages/knowledge/knowledge_page.rs` (extend)
**Estimated lines**: ~250 (added)

**Implement all CRUD handlers** called by the tree component:

1. **Create file** — `on_create_file(parent_id)`:
   - Open modal with title "New File"
   - On submit: call `create_knowledge_file(name, parent_id, None, false)`
   - Refresh tree entries
   - Select the newly created file

2. **Create folder** — `on_create_folder(parent_id)`:
   - Open modal with title "New Folder"
   - On submit: call `create_knowledge_file(name, parent_id, None, true)`
   - Refresh tree entries
   - Expand the parent folder

3. **Rename** — `on_rename(entry)`:
   - Open modal with title "Rename", default value = current name
   - On submit: call `update_knowledge_file(file_id, name=new_name)`
   - Refresh tree entries
   - Update selected file path if the renamed file is selected

4. **Delete** — `on_delete(file_id)`:
   - Show `ConfirmDialog`: "Delete [name]? This action cannot be undone."
   - For folders: warn "This will delete the folder and all its contents."
   - On confirm: call `delete_knowledge_file(file_id)`
   - Refresh tree entries
   - Clear selection if deleted file was selected

5. **Move** — `on_move(file_id, new_parent_id, sort_order)`:
   - Call `update_knowledge_file(file_id, parent_id=new_parent_id, sort_order)`
   - Refresh tree entries
   - Update selected file path if moved file is selected

6. **Tree refresh helper** — shared function that:
   - Calls `list_knowledge_tree()` server function
   - Updates `tree_entries` signal
   - Preserves expand state of folders

**Reference**: React `Knowledge.jsx` handlers and API calls.

**Verification**: `cargo check --workspace`, browser test: create file, rename, delete, move.

---

## Phase 7: Mobile Responsiveness

> **Why seventh**: The knowledge page has a distinct mobile layout that needs
> attention — sidebar collapses to an overlay.

### Task 7.1: Mobile Layout
**File**: `crates/kyomi-ui/src/pages/knowledge/knowledge_page.rs` (extend)
**File**: `crates/kyomi-ui/src/components/knowledge/file_tree.rs` (extend)
**Estimated lines**: ~120 (added across files)

**Implement**:
1. **Mobile detection** — viewport < 768px (same `use_is_mobile()` pattern from copilot_sidebar.rs)
2. **Mobile sidebar** — slide-in overlay panel:
   - Hidden by default, shown via hamburger/files button
   - Full-height overlay with backdrop
   - Auto-close on file selection
   - Transition animation (slide from left)
3. **Mobile header** — compact:
   - File name only (no breadcrumb path)
   - Hamburger button to toggle sidebar
   - Action buttons in overflow menu
4. **Mobile editor** — full width, no split panel
   - Preview below editor (stacked, not side-by-side)
   - Toolbar items wrap or collapse to overflow
5. **Toolbar metadata** — hide "Updated [time] by [name]" on mobile (already specified in React)

**Reference**: React `Knowledge.jsx` + `KnowledgeFileEditor.jsx` responsive classes.

**Verification**: `cargo check --workspace`, browser test: resize to mobile width.

---

## Phase 8: Route Wiring + Integration

> **Why last**: Wire everything together and verify end-to-end.

### Task 8.1: Route Registration + Module Setup
**File**: `crates/kyomi-ui/src/pages/knowledge/mod.rs` (create)
**File**: `crates/kyomi-ui/src/components/knowledge/mod.rs` (create)
**File**: `crates/kyomi-ui/src/pages/mod.rs` (modify)
**File**: `crates/kyomi-ui/src/components/mod.rs` (modify)
**File**: `crates/kyomi-ui/src/app.rs` (modify)
**File**: `crates/kyomi-ui/src/lib.rs` (modify — register server functions)

**Implement**:
1. Create `mod.rs` files exporting all knowledge modules
2. Register all new server functions in `lib.rs`
3. Update route in `app.rs`:
   ```rust
   <Route path=path!("/knowledge") view=|| view! { <Layout><KnowledgePage/></Layout> }/>
   ```
4. Remove `NotImplementedPage` reference for knowledge route

**Verification**: `cargo check --workspace`

### Task 8.2: Sidebar Integration
**File**: `crates/kyomi-ui/src/components/layout.rs` (modify)

**Implement**:
1. Update sidebar "Knowledge" link to navigate to `/knowledge`
2. Verify active state highlighting works when on `/knowledge` route

**Verification**: `cargo check --workspace`, browser test: sidebar link works, highlights correctly.

### Task 8.3: End-to-End Testing
**No file changes — verification only.**

**Test matrix** (must pass before declaring complete):

| Test Case | Steps | Expected |
|-----------|-------|----------|
| Page load | Navigate to `/knowledge` | File tree loads in sidebar, empty editor state |
| Create file | Click "New File", enter name | Modal opens, file created, appears in tree, auto-selected |
| Create folder | Click "New Folder", enter name | Modal opens, folder created, appears in tree |
| Create in folder | Right-click folder → New File | File created inside that folder |
| Select file | Click file in tree | Content loads in editor |
| Edit content | Type in editor | "Saving..." appears after 1.5s, then "Saved" |
| Conflict detection | Edit same file in two tabs | Second tab shows "Conflict!" with Reload button |
| Reload on conflict | Click "Reload?" | Fresh content loaded, conflict cleared |
| Rename file | Right-click → Rename | Modal with current name, updates on submit |
| Rename folder | Right-click folder → Rename | Folder name updates in tree |
| Delete file | Right-click → Delete | Confirm dialog, file removed from tree |
| Delete folder | Right-click folder → Delete | Warning about contents, folder + children removed |
| Move via context menu | Right-click → Move to → folder | File moves to target folder |
| Move via drag | Drag file onto folder | File moves, folder auto-expands |
| Drag prevention | Drag folder onto its own child | Drop rejected (no circular reference) |
| Search files | Type in search box | Results appear after 300ms, flat list with paths |
| Search clear | Click X on search | Returns to tree view |
| Search select | Click search result | File selected, parent folders expanded |
| Expand/collapse | Click folder | Toggles children visibility |
| Source mode | Default mode | Code editor with markdown highlighting |
| Visual mode toggle | Click "Visual" button | Shows "Visual editing coming soon" stub |
| Live preview | Edit markdown in source mode | Preview updates after 600ms |
| ChartML in preview | Add chartml block | Chart renders in preview panel |
| Mobile sidebar | Resize < 768px | Sidebar collapses to overlay |
| File path breadcrumb | Select nested file | Shows "Folder / SubFolder / File.md" |
| Empty folder | Select empty folder | Shows "Select a file to view its contents" |

---

## Estimated Totals

| Phase | Tasks | Est. New Lines | Files Created | Files Modified |
|-------|-------|---------------|---------------|----------------|
| 1. Server Functions | 2 | ~400 | 1 | 0 |
| 2. Tree Types | 1 | ~120 | 1 | 0 |
| 3. Create/Rename Modal | 1 | ~100 | 1 | 0 |
| 4. File Tree Sidebar | 4 | ~870 | 1 | 0 |
| 5. File Editor | 3 | ~530 | 1 | 0 |
| 6. Knowledge Page | 2 | ~600 | 1 | 0 |
| 7. Mobile Responsiveness | 1 | ~120 | 0 | 2 |
| 8. Integration + Testing | 3 | ~80 | 2 | 4 |
| **Total** | **17** | **~2,820** | **8** | **6** |

## Task Execution Order

Tasks within each phase are sequential. Phases can be parallelized where shown:

```
Phase 1 (Server Functions) ──────────────────────────────────────────────►
         │
         ├── Phase 2 (Tree Types) ──────────────────────────────────────►
         │         │
         │         ├── Phase 4 (File Tree) ─────────────────────────────►
         │         │
         │         └── Phase 6 (Knowledge Page) ────────────────────────►
         │                    │
         │                    ├── Phase 7 (Mobile) ─────────────────────►
         │                    │
         │                    └── Phase 8 (Integration) ────────────────►
         │
         ├── Phase 3 (Create/Rename Modal) ─────────────────────────────►
         │
         └── Phase 5 (File Editor) ─────────────────────────────────────►
```

## Critical Rules for Implementing Agents

1. **Match React source exactly** — copy CSS classes verbatim, match HTML structure
2. **No hacks, shortcuts, or mocks** — if auto-save doesn't work, fix it properly
3. **Read the React source AND the Rust backend** before writing any code
4. **Use existing Leptos patterns** — look at `dashboard_editor.rs` for editor, `dashboards_list.rs` for list pages
5. **Server functions call service layer directly** — don't HTTP-call REST endpoints
6. **Every task must end with `cargo check --workspace`** passing
7. **Don't skip features** — every context menu item, every drag state, every empty state
8. **Use Kode for code editing** — `kode-leptos` CodeEditor, not Monaco or Tiptap
9. **CAS conflict handling is critical** — test the 409 flow, it's a core feature
10. **Tree building happens client-side** — server returns flat list, `build_tree()` constructs hierarchy
