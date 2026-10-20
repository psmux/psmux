use std::io;
use ratatui::prelude::*;

use crate::types::{AppState, Pane, Node, LayoutKind, DragState};
use crate::platform::process_kill;

/// Split an area into sub-rects with 1px gaps between them for separator lines.
/// Matches tmux-style gapless panes with single-character separators.
pub fn split_with_gaps(is_horizontal: bool, sizes: &[u16], area: Rect) -> Vec<Rect> {
    let n = sizes.len();
    if n == 0 { return vec![]; }
    if n == 1 { return vec![area]; }

    let gaps = (n - 1) as u16;
    let total_available = if is_horizontal {
        area.width.saturating_sub(gaps)
    } else {
        area.height.saturating_sub(gaps)
    };

    let total_pct: u32 = sizes.iter().map(|&s| s as u32).sum();
    if total_pct == 0 { return vec![area; n]; }

    // Compute proportional sizes first.
    let mut child_sizes: Vec<u16> = Vec::with_capacity(n);
    let mut running: u16 = 0;
    for (i, &pct) in sizes.iter().enumerate() {
        let size = if i == n - 1 {
            total_available.saturating_sub(running)
        } else {
            let s = ((total_available as u32 * pct as u32) / total_pct) as u16;
            running = running.saturating_add(s);
            s
        };
        child_sizes.push(size);
    }

    // If total space allows the minimum for every child, guarantee it by
    // stealing from the largest siblings. This prevents previews of windows
    // with many nested splits from completely hiding deeply-nested panes when
    // the preview area is small, and on the horizontal axis it is what keeps a
    // cell from ever being one column wide: a pseudoconsole squeezed to one
    // column with a wide glyph in it never echoes again after it grows back
    // (see `crate::pane::MIN_PTY_COLS`). Rows keep tmux's minimum of one.
    let min_cell: u16 = if is_horizontal { crate::pane::MIN_PTY_COLS } else { 1 };
    if total_available >= n as u16 * min_cell {
        loop {
            let mut short_idx: Option<usize> = None;
            for (i, &s) in child_sizes.iter().enumerate() {
                if s < min_cell { short_idx = Some(i); break; }
            }
            let Some(si) = short_idx else { break };
            // Find the largest child that can spare a cell and stay at the minimum.
            let mut max_idx = 0usize;
            let mut max_val = 0u16;
            for (i, &s) in child_sizes.iter().enumerate() {
                if s > max_val { max_val = s; max_idx = i; }
            }
            if max_val <= min_cell { break; }
            child_sizes[max_idx] -= 1;
            child_sizes[si] += 1;
        }
    }

    let mut rects = Vec::with_capacity(n);
    let mut offset: u16 = 0;
    for (i, &size) in child_sizes.iter().enumerate() {
        let child_rect = if is_horizontal {
            Rect::new(area.x + offset + i as u16, area.y, size, area.height)
        } else {
            Rect::new(area.x, area.y + offset + i as u16, area.width, size)
        };
        rects.push(child_rect);
        offset += size;
    }

    rects
}

/// The narrowest cell the layout hands out on an axis: tmux's `PANE_MINIMUM`
/// (1) for rows, `MIN_PTY_COLS` for columns (see `split_with_gaps`).
fn min_cell(is_horizontal: bool) -> u16 {
    if is_horizontal { crate::pane::MIN_PTY_COLS } else { 1 }
}

/// Rewrite every split's `sizes` as the CELL extents its children have in
/// `area`, the representation tmux keeps (`layout_cell` sx/sy).
///
/// `sizes` are relative weights that `split_with_gaps` scales to the area, so
/// a 50/50 or percentage weight cannot express "move this border one cell":
/// adding 1 to a percent moved it 1.2 cells and rounded, which skipped 62 on
/// the way from 61 to 63 on a 120 column window. Once the weights ARE the
/// cells (and add up to the area less the borders) the scaling is exact, so
/// split and resize can do tmux's arithmetic on them directly. The geometry
/// does not change: these are exactly the rects already on screen.
pub fn normalize_sizes_to_cells(node: &mut Node, area: Rect) {
    if let Node::Split { kind, sizes, children } = node {
        if children.is_empty() { return; }
        let effective: Vec<u16> = if sizes.len() == children.len() { sizes.clone() }
            else { vec![(100 / children.len().max(1)) as u16; children.len()] };
        let is_h = matches!(*kind, LayoutKind::Horizontal);
        let rects = split_with_gaps(is_h, &effective, area);
        *sizes = rects.iter().map(|r| if is_h { r.width } else { r.height }).collect();
        for (child, r) in children.iter_mut().zip(rects.iter()) {
            normalize_sizes_to_cells(child, *r);
        }
    }
}

/// `normalize_sizes_to_cells` for a whole window, against the area it is
/// laid out in. A window that has not been laid out yet (zero area) keeps
/// its weights.
pub fn normalize_window_sizes(win: &mut crate::types::Window) {
    let area = win.area;
    // A zoomed window's weights are the 100/0 zoom mask, not its layout.
    if area.width == 0 || area.height == 0 || win.zoom_saved.is_some() { return; }
    normalize_sizes_to_cells(&mut win.root, area);
}

/// tmux `layout_split_sizes` (layout.c): `ss` is the cell being split, `size`
/// the requested size of the NEW pane (`-l`), or None for the middle split.
/// Returns (top/left, bottom/right).
///
///     s2 = size < 0 ? (ss + 1) / 2 - 1 : before ? ss - size - 1 : size
///     s2 clamped to [PANE_MINIMUM, ss - 2],  s1 = ss - 1 - s2
///
/// so an even cell gives the first pane the extra cell (120 -> 60 | 59) and
/// `-b -l N` makes the new, first, pane N wide.
pub fn tmux_split_sizes(ss: u16, size: Option<u16>, before: bool, min: u16) -> (u16, u16) {
    let ss = ss as i32;
    let mut s2 = match size {
        None => (ss + 1) / 2 - 1,
        Some(n) if before => ss - n as i32 - 1,
        Some(n) => n as i32,
    };
    if s2 < min as i32 { s2 = min as i32; } else if s2 > ss - 2 { s2 = ss - 2; }
    let s2 = s2.max(0);
    let s1 = (ss - 1 - s2).max(0);
    (s1 as u16, s2 as u16)
}

/// Split the leaf at `path` the way tmux's `layout_split_pane` does and return
/// the new leaf's path.
///
/// * sizes come from `tmux_split_sizes` on the leaf's current cell, so the
///   split is in cells, not halves of a percentage;
/// * `before` (`-b`, SPAWN_BEFORE) puts the new pane top/left of the old one;
/// * when the leaf's parent is already a split of the same kind, the new cell
///   is inserted into that parent beside the leaf (layout.c: "If the parent
///   exists and is of the same type as the split, create a new cell and insert
///   it after this one") instead of nesting a two pane split inside it, which
///   is what `#{window_layout}` and every later resize see.
///
/// `size` is `-l`: (cells, false) or (percent, true). `area` is the window's area. On a path that does not name a leaf the new
/// leaf is killed (its ConPTY would leak) and None comes back.
pub fn split_leaf_tmux(root: &mut Node, path: &[usize], area: Rect, kind: LayoutKind, new_leaf: Node, size: Option<(u16, bool)>, before: bool) -> Option<Vec<usize>> {
    // A path through a leaf, or past the end, names nothing to split: the
    // new pane is killed so its ConPTY does not leak. (A path naming a split
    // wraps that split, as replace_leaf_with_split always did.)
    if get_node(root, path).is_none() {
        kill_node(new_leaf);
        return None;
    }
    normalize_sizes_to_cells(root, area);
    let is_h = matches!(kind, LayoutKind::Horizontal);
    let cell = node_rect(root, area, path).unwrap_or(area);
    let ss = if is_h { cell.width } else { cell.height };
    // `-l N%` is a share of the pane being split (cmd-split-window.c takes
    // curval from wp->sx / wp->sy), truncated as args_percentage does.
    let size = size.map(|(v, pct)| if pct { (ss as u32 * v as u32 / 100) as u16 } else { v });
    let (s1, s2) = tmux_split_sizes(ss, size, before, min_cell(is_h));
    let (old_size, new_size) = if before { (s2, s1) } else { (s1, s2) };

    if let Some((&idx, parent_path)) = path.split_last() {
        if let Some(Node::Split { kind: pk, sizes, children }) = get_split_mut(root, &parent_path.to_vec()) {
            if *pk == kind && sizes.len() == children.len() {
                let at = if before { idx } else { idx + 1 };
                sizes[idx] = old_size;
                sizes.insert(at, new_size);
                children.insert(at, new_leaf);
                let mut np = parent_path.to_vec();
                np.push(at);
                return Some(np);
            }
        }
    }
    let target = node_at_mut(root, path)?;
    let old = std::mem::replace(target, Node::Split { kind, sizes: vec![], children: vec![] });
    if let Node::Split { sizes, children, .. } = target {
        if before {
            children.push(new_leaf); children.push(old);
            *sizes = vec![new_size, old_size];
        } else {
            children.push(old); children.push(new_leaf);
            *sizes = vec![old_size, new_size];
        }
    }
    let mut np = path.to_vec();
    np.push(if before { 0 } else { 1 });
    Some(np)
}

/// `split-window -f` (SPAWN_FULLSIZE in layout_split_pane): the new pane
/// takes a full width column or full height row of the window, the existing
/// layout shrinks into the rest. When the root already splits the same way the
/// new cell joins it (its children are resized into the old share first);
/// otherwise the root is wrapped in a new split. Returns the new leaf's path.
pub fn split_full_tmux(root: &mut Node, area: Rect, kind: LayoutKind, new_leaf: Node, size: Option<(u16, bool)>, before: bool) -> Vec<usize> {
    normalize_sizes_to_cells(root, area);
    let is_h = matches!(kind, LayoutKind::Horizontal);
    let ss = if is_h { area.width } else { area.height };
    // `-f -l N%` is a share of the window (cmd-split-window.c curval = w->sx).
    let size = size.map(|(v, pct)| if pct { (ss as u32 * v as u32 / 100) as u16 } else { v });
    let (s1, s2) = tmux_split_sizes(ss, size, before, min_cell(is_h));
    let (old_size, new_size) = if before { (s2, s1) } else { (s1, s2) };
    if let Node::Split { kind: rk, sizes, children } = root {
        if *rk == kind && sizes.len() == children.len() && !children.is_empty() {
            // layout_resize_child_cells into old_size, then the new cell.
            let shrunk = if is_h { Rect::new(0, 0, old_size, area.height) } else { Rect::new(0, 0, area.width, old_size) };
            let rects = split_with_gaps(is_h, sizes, shrunk);
            *sizes = rects.iter().map(|r| if is_h { r.width } else { r.height }).collect();
            if before {
                sizes.insert(0, new_size);
                children.insert(0, new_leaf);
                return vec![0];
            }
            sizes.push(new_size);
            children.push(new_leaf);
            return vec![children.len() - 1];
        }
    }
    let old = std::mem::replace(root, Node::Split { kind, sizes: vec![], children: vec![] });
    if let Node::Split { sizes, children, .. } = root {
        if before {
            children.push(new_leaf); children.push(old);
            *sizes = vec![new_size, old_size];
        } else {
            children.push(old); children.push(new_leaf);
            *sizes = vec![old_size, new_size];
        }
    }
    vec![if before { 0 } else { 1 }]
}

/// The rect the node at `path` occupies when `root` is laid out in `area`.
pub fn node_rect(root: &Node, area: Rect, path: &[usize]) -> Option<Rect> {
    let mut cur = root;
    let mut rect = area;
    for &i in path {
        match cur {
            Node::Split { kind, sizes, children } => {
                let effective: Vec<u16> = if sizes.len() == children.len() { sizes.clone() }
                    else { vec![(100 / children.len().max(1)) as u16; children.len()] };
                let rects = split_with_gaps(matches!(*kind, LayoutKind::Horizontal), &effective, rect);
                rect = *rects.get(i)?;
                cur = children.get(i)?;
            }
            Node::Leaf(_) => return None,
        }
    }
    Some(rect)
}

/// Drop `sizes[idx]` and give its cells, and the border that went with it, to
/// the cell that takes the space in tmux's `layout_destroy_cell`: the previous
/// sibling, or the next one when the first cell goes. Equalising the survivors
/// (or rescaling them to 100) moved every other border in the window.
pub fn merge_freed_size(sizes: &mut Vec<u16>, idx: usize) {
    if idx >= sizes.len() { return; }
    let freed = sizes.remove(idx);
    if sizes.is_empty() { return; }
    let other = if idx == 0 { 0 } else { idx - 1 };
    sizes[other] = sizes[other].saturating_add(freed).saturating_add(1);
}

pub fn active_pane_mut<'a>(node: &'a mut Node, path: &Vec<usize>) -> Option<&'a mut Pane> {
    let mut cur = node;
    for &idx in path.iter() {
        match cur {
            Node::Split { children, .. } => { cur = children.get_mut(idx)?; }
            Node::Leaf(_) => return None,
        }
    }
    match cur { Node::Leaf(p) => Some(p), _ => None }
}

pub fn replace_leaf_with_split(node: &mut Node, path: &Vec<usize>, kind: LayoutKind, new_leaf: Node) {
    if path.is_empty() {
        let old = std::mem::replace(node, Node::Split { kind, sizes: vec![50,50], children: vec![] });
        if let Node::Split { children, .. } = node { children.push(old); children.push(new_leaf); }
        return;
    }
    let mut cur = node;
    for (depth, &idx) in path.iter().enumerate() {
        match cur {
            Node::Split { children, .. } => {
                if depth == path.len()-1 {
                    let leaf = std::mem::replace(&mut children[idx], Node::Split { kind, sizes: vec![50,50], children: vec![] });
                    if let Node::Split { children: c, .. } = &mut children[idx] { c.push(leaf); c.push(new_leaf); }
                    return;
                } else { cur = &mut children[idx]; }
            }
            Node::Leaf(_) => {
                // Path is invalid (points through a Leaf). Kill the new pane
                // to prevent leaking its ConPTY handle and reader thread.
                kill_node(new_leaf);
                return;
            },
        }
    }
}

/// `replace_leaf_with_split` with the new leaf placed FIRST when `before` is
/// set (join-pane / move-pane `-b`, cmd-join-pane.c `SPAWN_BEFORE`).
pub fn replace_leaf_with_split_ordered(node: &mut Node, path: &Vec<usize>, kind: LayoutKind, new_leaf: Node, before: bool) {
    replace_leaf_with_split(node, path, kind, new_leaf);
    if !before { return; }
    if let Some(Node::Split { children, .. }) = get_split_mut(node, path) {
        if children.len() == 2 { children.swap(0, 1); }
    }
}

/// True when `win.active_path` names a pane, not a split. Every consumer that
/// walks the path to a `Pane` (key input, paste, copy mode) gets `None` for a
/// split, so a window in that state has no keyboard focus at all (#725).
pub fn active_path_is_leaf(win: &crate::types::Window) -> bool {
    matches!(get_node(&win.root, &win.active_path), Some(Node::Leaf(_)))
}

fn get_node<'a>(node: &'a Node, path: &[usize]) -> Option<&'a Node> {
    let mut cur = node;
    for &idx in path {
        match cur { Node::Split { children, .. } => { cur = children.get(idx)?; } Node::Leaf(_) => return None }
    }
    Some(cur)
}

/// Where a pane grafted by `graft_pane` ended up.
pub struct Grafted {
    /// Path of the grafted pane in the target window.
    pub new_path: Vec<usize>,
    /// Path of the pane that was active in the target window before the graft,
    /// re-found by id because the graft moved it one level down.
    pub prev_active_path: Option<Vec<usize>>,
}

/// Graft `new_leaf` beside the pane at `path` of `win` (join-pane, move-pane,
/// cross session join), then re-anchor `win.active_path` on a LEAF.
///
/// The graft turns the leaf at `path` into a split, so any `active_path` that
/// pointed at it (the default target) now names a split node. Left that way the
/// window had no keyboard focus: typed keys reached no pane until something
/// re-focused one by id (#725). tmux makes the joined pane active unless `-d`
/// (cmd-join-pane.c, `window_set_active_pane(dst_w, src_wp, 1)` inside
/// `if (!args_has(args, 'd'))`), so `focus_new` selects it, and otherwise the
/// pane that was active before keeps the focus.
pub fn graft_pane(win: &mut crate::types::Window, path: &Vec<usize>, kind: LayoutKind, new_leaf: Node, before: bool, focus_new: bool) -> Option<Grafted> {
    graft_pane_sized(win, path, kind, new_leaf, None, before, focus_new)
}

/// graft_pane with join-pane / move-pane `-l` (`size`: cells, or a share of
/// the target pane when the flag is set), as layout_get_tiled_cell sizes it.
pub fn graft_pane_sized(win: &mut crate::types::Window, path: &Vec<usize>, kind: LayoutKind, new_leaf: Node,
                        size: Option<(u16, bool)>, before: bool, focus_new: bool) -> Option<Grafted> {
    let new_id = match &new_leaf { Node::Leaf(p) => Some(p.id), Node::Split { .. } => collect_pane_ids(&new_leaf).first().copied() };
    let prev_id = get_active_pane_id(&win.root, &win.active_path);
    // cmd-join-pane.c splits the target with the same layout_split_pane as
    // split-window, so the joined pane gets the same cells (60 | 59 on 120).
    if win.area.width > 0 && win.area.height > 0 {
        let area = win.area;
        split_leaf_tmux(&mut win.root, path, area, kind, new_leaf, size, before)?;
    } else {
        replace_leaf_with_split_ordered(&mut win.root, path, kind, new_leaf, before);
    }
    let new_path = new_id.and_then(|id| find_path_by_id(&win.root, id))?;
    let prev_active_path = prev_id.and_then(|id| find_path_by_id(&win.root, id));
    if focus_new {
        win.active_path = new_path.clone();
        if let Some(id) = new_id { touch_mru(&mut win.pane_mru, id); }
    } else {
        win.active_path = prev_active_path.clone().unwrap_or_else(|| first_leaf_path(&win.root));
    }
    debug_assert!(active_path_is_leaf(win), "graft_pane left active_path {:?} on a split", win.active_path);
    Some(Grafted { new_path, prev_active_path })
}

pub fn kill_leaf(node: &mut Node, path: &Vec<usize>) {
    *node = remove_node(std::mem::replace(node, Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] }), path);
}

/// Kill a node and all its child processes before dropping it.
/// Uses platform-specific process tree killing to ensure all descendant
/// processes (shells, sub-processes, servers, etc.) are terminated.
pub fn kill_node(mut n: Node) {
    match &mut n {
        Node::Leaf(p) => { process_kill::kill_process_tree(&mut p.child); }
        Node::Split { children, .. } => {
            for child in children.iter_mut() {
                kill_all_children(child);
            }
        }
    }
}

pub fn remove_node(n: Node, path: &Vec<usize>) -> Node {
    match n {
        Node::Leaf(p) => {
            Node::Leaf(p)
        }
        Node::Split { kind, mut sizes, children } => {
            if path.is_empty() { return Node::Split { kind, sizes, children }; }
            let idx = path[0];
            let n = children.len();
            let mut new_children: Vec<Node> = Vec::new();
            for (i, child) in children.into_iter().enumerate() {
                if i == idx {
                    if path.len() > 1 { new_children.push(remove_node(child, &path[1..].to_vec())); }
                    else {
                        kill_node(child);
                    }
                } else { new_children.push(child); }
            }
            if new_children.len() == 1 { new_children.into_iter().next().unwrap() }
            else {
                let sizes = survivor_sizes(&mut sizes, n, idx, new_children.len());
                Node::Split { kind, sizes, children: new_children }
            }
        }
    }
}

/// The sizes of a split after child `idx` left it (`n` children before,
/// `left` after). A child that went away hands its cells to its neighbour
/// (`merge_freed_size`); a child that stayed (the removal was deeper down)
/// keeps every size as it was.
fn survivor_sizes(sizes: &mut Vec<u16>, n: usize, idx: usize, left: usize) -> Vec<u16> {
    if sizes.len() == n && left + 1 == n {
        merge_freed_size(sizes, idx);
        return sizes.clone();
    }
    if sizes.len() == left { return sizes.clone(); }
    let mut eq = vec![100 / left.max(1) as u16; left];
    let rem = 100u16.saturating_sub(eq.iter().sum::<u16>());
    if let Some(last) = eq.last_mut() { *last += rem; }
    eq
}

/// Extract (detach) a node from the tree at the given path WITHOUT killing it.
/// Returns (remaining_tree, extracted_node).
/// If the path points to the root, returns (None, root).
pub fn extract_node(root: Node, path: &[usize]) -> (Option<Node>, Option<Node>) {
    if path.is_empty() {
        return (None, Some(root));
    }
    match root {
        Node::Leaf(p) => (Some(Node::Leaf(p)), None), // path doesn't exist
        Node::Split { kind, mut sizes, children } => {
            let idx = path[0];
            let n = children.len();
            if idx >= children.len() {
                return (Some(Node::Split { kind, sizes, children }), None);
            }
            if path.len() == 1 {
                // Extract child at idx
                let mut remaining: Vec<Node> = Vec::new();
                let mut extracted: Option<Node> = None;
                for (i, child) in children.into_iter().enumerate() {
                    if i == idx { extracted = Some(child); }
                    else { remaining.push(child); }
                }
                let tree = if remaining.is_empty() {
                    None
                } else if remaining.len() == 1 {
                    Some(remaining.into_iter().next().unwrap())
                } else {
                    let sizes = survivor_sizes(&mut sizes, n, idx, remaining.len());
                    Some(Node::Split { kind, sizes, children: remaining })
                };
                (tree, extracted)
            } else {
                // Recurse into the child at idx
                let mut new_children: Vec<Node> = Vec::new();
                let mut extracted: Option<Node> = None;
                for (i, child) in children.into_iter().enumerate() {
                    if i == idx {
                        let (rem, ext) = extract_node(child, &path[1..]);
                        extracted = ext;
                        if let Some(r) = rem { new_children.push(r); }
                    } else {
                        new_children.push(child);
                    }
                }
                let tree = if new_children.is_empty() {
                    None
                } else if new_children.len() == 1 {
                    Some(new_children.into_iter().next().unwrap())
                } else {
                    let sizes = survivor_sizes(&mut sizes, n, idx, new_children.len());
                    Some(Node::Split { kind, sizes, children: new_children })
                };
                (tree, extracted)
            }
        }
    }
}

pub fn compute_rects(node: &Node, area: Rect, out: &mut Vec<(Vec<usize>, Rect)>) {
    fn rec(node: &Node, area: Rect, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, Rect)>) {
        match node {
            Node::Leaf(_) => { out.push((path.clone(), area)); }
            Node::Split { kind, sizes, children } => {
                let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                    sizes.clone()
                } else { vec![(100 / children.len().max(1)) as u16; children.len()] };
                let is_horizontal = matches!(*kind, LayoutKind::Horizontal);
                let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
                for (i, child) in children.iter().enumerate() {
                    if i < rects.len() { path.push(i); rec(child, rects[i], path, out); path.pop(); }
                }
            }
        }
    }
    let mut path = Vec::new();
    rec(node, area, &mut path, out);
}

/// The rects `#{pane_left}`, `#{pane_top}` and friends report: the layout's,
/// and while the window is zoomed, the layout it had before the zoom with
/// the zoomed pane over the whole window.
///
/// tmux (window_zoom) lays out only the zoomed pane and leaves every other
/// pane's xoff/yoff/sx/sy as they were, so list-panes on a zoomed window
/// shows the zoomed pane at 0,0 at full size and the rest where they were.
/// The weights during a zoom are the 100/0 mask, so computing from them put
/// the zoomed pane at x 3 and squeezed the hidden ones into two column
/// slivers at the edge.
pub fn format_rects(win: &crate::types::Window, out: &mut Vec<(Vec<usize>, Rect)>) {
    let Some(saved) = win.zoom_saved.as_ref() else {
        compute_rects(&win.root, win.area, out);
        return;
    };
    fn rec(node: &Node, area: Rect, path: &mut Vec<usize>, saved: &[(Vec<usize>, Vec<u16>)], out: &mut Vec<(Vec<usize>, Rect)>) {
        match node {
            Node::Leaf(_) => out.push((path.clone(), area)),
            Node::Split { kind, sizes, children } => {
                let base = saved.iter().find(|(p, _)| p == path).map(|(_, s)| s).unwrap_or(sizes);
                let effective: Vec<u16> = if base.len() == children.len() { base.clone() }
                    else { vec![(100 / children.len().max(1)) as u16; children.len()] };
                let rects = split_with_gaps(matches!(*kind, LayoutKind::Horizontal), &effective, area);
                for (i, child) in children.iter().enumerate() {
                    if i < rects.len() { path.push(i); rec(child, rects[i], path, saved, out); path.pop(); }
                }
            }
        }
    }
    let mut path = Vec::new();
    rec(&win.root, win.area, &mut path, saved, out);
    if let Some((_, r)) = out.iter_mut().find(|(p, _)| *p == win.active_path) {
        *r = win.area;
    }
}

/// The pseudoconsole size a pane gets for the layout slot it owns.
///
/// `border_rows` is the row `pane-border-status` takes out of the slot for the
/// pane's label (#288).
///
/// The rule is tmux's: a pane is exactly as big as its cell.  tmux keeps no
/// second opinion about a pane's size, it just calls `window_pane_resize` with
/// the cell's `sx`/`sy` (tmux window.c:1564) and its floor is `PANE_MINIMUM 1`
/// (tmux.h:110), so a one row cell holds a one row pane.
///
/// This used to clamp to `MIN_PANE_DIM` (2) instead, which is what #644 was: a
/// slot one row tall got a two row screen, `pane_height` said 2 while
/// `pane_top` and `pane_bottom` were both 0, and the client, seeing a source
/// taller than the rect it had, went down its oversized preview path and
/// painted the screen's blank second row into the only row on offer.  The
/// program kept running and `capture-pane` kept showing live content, so the
/// pane looked healthy from every server side check while the client drew
/// nothing at all.
///
/// Width is the one axis with a floor above one: `MIN_PTY_COLS` (2), because a
/// one column pseudoconsole holding a wide glyph never echoes again after it is
/// grown back (issue #534's suite caught this the day the floor was dropped to
/// one). `split_with_gaps` applies the same floor to the cell, so the slot and
/// the pane still agree.
#[must_use]
pub fn pane_inner_size(rect: Rect, border_rows: u16) -> (u16, u16) {
    let height = rect.height.saturating_sub(border_rows).max(crate::pane::MIN_PTY_DIM);
    let width = rect.width.max(crate::pane::MIN_PTY_COLS);
    (height, width)
}

/// Resize all panes in one window to match the supplied window area.
pub fn resize_window_panes(app: &mut AppState, window_index: usize, area: Rect) {
    if window_index >= app.windows.len() || area.width == 0 || area.height == 0 { return; }
    // Reserve 1 row per leaf pane when pane-border-status is enabled (#288)
    let border_status_rows: u16 = match app.user_options.get("pane-border-status").map(|s| s.as_str()) {
        Some("top") | Some("bottom") => 1,
        _ => 0,
    };
    
    fn resize_node(node: &mut Node, rects: &[(Vec<usize>, Rect)], path: &mut Vec<usize>, border_rows: u16, zoom_active_path: Option<&Vec<usize>>) {
        match node {
            Node::Leaf(pane) => {
                // Skip resize for panes hidden by zoom. `split_with_gaps`'s
                // minimum-1-cell steal (added for window-preview thumbnails)
                // means a hidden sibling's computed rect is NOT reliably 0x0
                // any more — it can be stolen up to 1-2 cells — so checking
                // `rect.width == 0 || rect.height == 0` alone no longer
                // detects every zoomed-out pane. Compare against the actual
                // zoom invariant instead: any leaf whose path isn't exactly
                // the zoomed window's active_path is on a hidden branch
                // (fixes #44, #45 — resizing a hidden pane corrupts its
                // terminal buffer: lines get reflowed to 1-2 column width
                // and the cursor position is lost).
                if let Some(ap) = zoom_active_path {
                    if path != ap { return; }
                }
                if let Some((_, rect)) = rects.iter().find(|(p, _)| p == path) {
                    // Fallback/legacy guard: also skip on a literal 0x0 rect.
                    if rect.width == 0 || rect.height == 0 {
                        return;
                    }
                    // Size the pane to the slot the layout actually gave it
                    // (#644).  See `pane_inner_size` for why nothing is
                    // rounded up here.
                    let (inner_height, inner_width) = pane_inner_size(*rect, border_rows);

                    if pane.last_rows != inner_height || pane.last_cols != inner_width {
                        let _ = pane.master.resize(portable_pty::PtySize {
                            rows: inner_height,
                            cols: inner_width,
                            pixel_width: 0,
                            pixel_height: 0
                        });
                        // Both screens: the visible one (a copy-mode snapshot
                        // while copy mode is up) and the live one behind it.
                        for term in pane.each_term() {
                            if let Ok(mut parser) = term.lock() {
                                parser.screen_mut().set_size(inner_height, inner_width);
                            }
                        }
                        pane.last_rows = inner_height;
                        pane.last_cols = inner_width;
                    }
                }
            }
            Node::Split { children, .. } => {
                for (i, child) in children.iter_mut().enumerate() {
                    path.push(i);
                    resize_node(child, rects, path, border_rows, zoom_active_path);
                    path.pop();
                }
            }
        }
    }
    
    // A resize can trim retained scrollback. A copy-mode pane counts its view
    // in lines above the live bottom, so remember what it was showing (offset
    // plus the retained depth it was measured against) and re-anchor after the
    // panes have their new size. Without this the view lands on the oldest
    // retained line and stays there until Esc (see
    // copy_mode::reanchor_after_resize).
    // `copy_scroll_offset` and `reanchor_after_resize` both address the ACTIVE
    // window's active pane, so only that window's resize may re-anchor: a
    // background window resizing (resize_all_panes walks every window) would
    // otherwise measure its own pane's retained depth and apply the shift to
    // the pane the user is actually looking at.
    let copy_view_before = if window_index == app.active_idx
        && app.mode.in_copy()
    {
        let win = &app.windows[window_index];
        active_pane(&win.root, &win.active_path).map(|p| {
            let filled = p.term.lock().map(|t| t.screen().scrollback_filled()).unwrap_or(0);
            (app.copy_scroll_offset, filled)
        })
    } else {
        None
    };

    let win = &mut app.windows[window_index];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, area, &mut rects);
    // When the window is zoomed, split_with_gaps still subtracts the
    // separator gap (1 px) AND the minimum-size steal (1 px) from the
    // visible pane, making it 2 rows/cols shorter than the full viewport.
    // The client renders the zoomed pane using the full area, so the PTY
    // must also be sized to the full area — otherwise the bottom/right
    // edge shows blank rows/columns.
    let zoom_active_path = if win.zoom_saved.is_some() {
        let active_path = win.active_path.clone();
        if let Some((_, rect)) = rects.iter_mut().find(|(p, _)| *p == active_path) {
            *rect = area;
        }
        Some(active_path)
    } else {
        None
    };
    let mut path = Vec::new();
    resize_node(&mut win.root, &rects, &mut path, border_status_rows, zoom_active_path.as_ref());

    if let Some((offset_before, filled_before)) = copy_view_before {
        crate::copy_mode::reanchor_after_resize(app, offset_before, filled_before);
    }
}

/// Resize the active window. Its stored geometry is authoritative, including
/// when a previous `resize-window` put it in manual mode.
pub fn resize_all_panes(app: &mut AppState) {
    if app.active_idx >= app.windows.len() { return; }
    let area = app.windows[app.active_idx].area;
    app.last_window_area = area;
    resize_window_panes(app, app.active_idx, area);
}

pub fn kill_all_children(node: &mut Node) {
    match node {
        Node::Leaf(p) => { process_kill::kill_process_tree(&mut p.child); }
        Node::Split { children, .. } => { for child in children.iter_mut() { kill_all_children(child); } }
    }
}

/// Collect mutable references to all child processes in a tree node.
fn collect_child_refs<'a>(node: &'a mut Node, out: &mut Vec<&'a mut Box<dyn portable_pty::Child>>) {
    match node {
        Node::Leaf(p) => { out.push(&mut p.child); }
        Node::Split { children, .. } => { for child in children.iter_mut() { collect_child_refs(child, out); } }
    }
}

/// Kill all children across multiple windows using a single process snapshot.
/// Much faster than per-window `kill_all_children` when killing an entire session.
pub fn kill_all_children_batch(windows: &mut [crate::types::Window]) {
    let mut all_children: Vec<&mut Box<dyn portable_pty::Child>> = Vec::new();
    for win in windows.iter_mut() {
        collect_child_refs(&mut win.root, &mut all_children);
    }
    if !all_children.is_empty() {
        process_kill::kill_process_trees_batch(&mut all_children);
    }
}

/// Returns borders as (path, kind, idx, pixel_pos, total_pixels_along_axis).
pub fn compute_split_borders(node: &Node, area: Rect, out: &mut Vec<(Vec<usize>, LayoutKind, usize, u16, u16)>) {
    fn rec(node: &Node, area: Rect, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, LayoutKind, usize, u16, u16)>) {
        match node {
            Node::Leaf(_) => {}
            Node::Split { kind, sizes, children } => {
                let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                    sizes.clone()
                } else { vec![(100 / children.len().max(1)) as u16; children.len()] };
                let is_horizontal = matches!(*kind, LayoutKind::Horizontal);
                let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
                let total_px = if is_horizontal { area.width } else { area.height };
                for i in 0..children.len().saturating_sub(1) {
                    if i < rects.len() {
                        let pos = if is_horizontal {
                            rects[i].x + rects[i].width
                        } else {
                            rects[i].y + rects[i].height
                        };
                        out.push((path.clone(), *kind, i, pos, total_px));
                    }
                }
                for (i, child) in children.iter().enumerate() {
                    if i < rects.len() { path.push(i); rec(child, rects[i], path, out); path.pop(); }
                }
            }
        }
    }
    let mut path = Vec::new();
    rec(node, area, &mut path, out);
}

pub fn split_sizes_at<'a>(node: &'a Node, path: Vec<usize>, idx: usize) -> Option<(u16,u16)> {
    let mut cur = node;
    for &i in path.iter() {
        match cur { Node::Split { children, .. } => { cur = children.get(i)?; } _ => return None }
    }
    if let Node::Split { sizes, .. } = cur {
        if idx+1 < sizes.len() { Some((sizes[idx], sizes[idx+1])) } else { None }
    } else { None }
}

/// Move one separator in relative-weight space. `pixels` includes the node's
/// separators, just as compute_split_borders and the client border snapshot do.
pub fn drag_split_sizes(sizes: &mut [u16], index: usize, left_initial: u16, delta: i32, pixels: u16, horizontal: bool) {
    let Some((&left, &right)) = sizes.get(index).zip(index.checked_add(1).and_then(|i| sizes.get(i))) else { return };
    // Zero weights can be a zoom mask; dragging must not reveal hidden panes.
    if left == 0 || right == 0 { return; }
    let available = i64::from(pixels) - (sizes.len() - 1) as i64;
    if available <= 0 { return; }
    let total: i64 = sizes.iter().map(|&s| i64::from(s)).sum();
    let pair = i64::from(left) + i64::from(right);
    let change = i64::from(delta) * total / available;
    // Preserve the five-percent drag floor and the layout's cell minimum.
    // A pane already below that floor must still return to its press position.
    let cell_floor = (i64::from(min_cell(horizontal)) * total + available - 1) / available;
    let floor = (total / 20).max(cell_floor).max(1);
    let initial = i64::from(left_initial).clamp(1, pair - 1);
    let minimum = floor.min(initial).max(pair - i64::from(u16::MAX));
    let maximum = (pair - floor.min(pair - initial)).min(i64::from(u16::MAX));
    let left = (i64::from(left_initial) + change).clamp(minimum, maximum);
    sizes[index] = left as u16;
    sizes[index + 1] = (pair - left) as u16;
}

pub fn adjust_split_sizes(root: &mut Node, d: &DragState, x: u16, y: u16) {
    if let Some(Node::Split { sizes, .. }) = get_split_mut(root, &d.split_path) {
        let pixel_delta: i32 = match d.kind {
            LayoutKind::Horizontal => x as i32 - d.start_x as i32,
            LayoutKind::Vertical => y as i32 - d.start_y as i32,
        };
        drag_split_sizes(sizes, d.index, d.left_initial, pixel_delta, d.total_pixels, matches!(d.kind, LayoutKind::Horizontal));
    }
}

pub fn get_split_mut<'a>(node: &'a mut Node, path: &Vec<usize>) -> Option<&'a mut Node> {
    let mut cur = node;
    for &idx in path.iter() {
        match cur { Node::Split { children, .. } => { cur = children.get_mut(idx)?; } _ => return None }
    }
    Some(cur)
}

/// Prune exited panes from the tree.  Returns `(Option<Node>, newly_dead_count)`:
/// - `newly_dead_count` tracks panes that transitioned alive→dead in this call
///   (remain-on-exit case), so callers can fire hooks even when the tree shape
///   doesn't change.
/// The whole `remain-on-exit` decision for one pane that just exited.
///
/// The chain is pane, then window, then global, exactly as tmux resolves an
/// option (options.c walks the pane table, the window table, then the global
/// window table). `pane` is the pane's own `set-option -p` entry, `window` is
/// what `set-option -w` resolved to for the window that holds it (#648) with
/// the session-wide value as its parent, and `failed` keeps the pane only when
/// the process exited nonzero — what a supervisor wants: crashed panes stay
/// visible with their error, clean exits close.
pub fn keep_dead_pane(pane: Option<&str>, window: bool, exit_success: bool) -> bool {
    match pane {
        Some("on") => true,
        Some("off") => false,
        Some("failed") => !exit_success,
        _ => window,
    }
}

pub fn prune_exited(n: Node, remain_on_exit: bool, kill_descendants: bool) -> (Option<Node>, usize) {
    match n {
        Node::Leaf(mut p) => {
            if p.dead { return (Some(Node::Leaf(p)), 0); }
            match p.child.try_wait() {
                Ok(Some(status)) => {
                    // Pane-scoped remain-on-exit overrides the session global
                    // (issue #580; tmux pane-option semantics): `on` keeps the
                    // dead pane, `off` closes it, `failed` keeps it only when
                    // the process exited nonzero — which is exactly what a
                    // teammate supervisor wants: crashed panes stay visible
                    // with their error, clean exits close.
                    let keep = keep_dead_pane(
                        p.pane_options.get("remain-on-exit").map(String::as_str),
                        remain_on_exit,
                        status.success(),
                    );
                    if keep {
                        if let Ok(mut parser) = p.live_parser().lock() {
                            parser.screen_mut().program_status_mut().expire();
                        }
                        p.dead = true;
                        (Some(Node::Leaf(p)), 1)
                    } else {
                        // Shell exited on its own: sweep any orphaned descendants
                        // (backgrounded child processes) before dropping the pane.
                        // Closing the ConPTY alone does NOT terminate grandchildren,
                        // so without this they leak. Mirrors the explicit kill-pane
                        // path and the reaper case kill_process_tree documents.
                        // `set -g @kill-descendants off` opts out, restoring
                        // tmux-on-Unix survival for deliberately backgrounded
                        // processes (see AppState::kill_descendants_on_exit).
                        if kill_descendants {
                            crate::platform::process_kill::kill_process_tree(&mut p.child);
                        }
                        (None, 0)
                    }
                }
                _ => (Some(Node::Leaf(p)), 0),
            }
        }
        Node::Split { kind, sizes, children } => {
            let mut new_children: Vec<Node> = Vec::new();
            let mut new_sizes: Vec<u16> = if sizes.len() == children.len() { sizes.clone() }
                else { vec![(100 / children.len().max(1)) as u16; children.len()] };
            let mut newly_dead = 0;
            let mut kept: Vec<Option<Node>> = Vec::new();
            for child in children.into_iter() {
                let (pruned, dead_count) = prune_exited(child, remain_on_exit, kill_descendants);
                newly_dead += dead_count;
                kept.push(pruned);
            }
            // Back to front, so each gone child's index into `new_sizes` is
            // still its own when its cells go to its neighbour (tmux
            // layout_destroy_cell). The other borders stay where they were.
            for i in (0..kept.len()).rev() {
                if kept[i].is_none() && new_sizes.len() > 1 { merge_freed_size(&mut new_sizes, i); }
            }
            for c in kept.into_iter().flatten() { new_children.push(c); }
            if new_children.is_empty() { (None, newly_dead) }
            else if new_children.len() == 1 { (Some(new_children.remove(0)), newly_dead) }
            else if new_sizes.len() == new_children.len() && new_sizes.iter().any(|&s| s > 0) {
                (Some(Node::Split { kind, sizes: new_sizes, children: new_children }), newly_dead)
            }
            else {
                // Redistribute removed pane's percentage proportionally among survivors
                let total: u16 = new_sizes.iter().sum();
                if total == 0 || total == 100 {
                    // Already fine or all zero — just normalize
                    if total == 0 {
                        new_sizes = vec![100 / new_children.len() as u16; new_children.len()];
                        let rem = 100 - new_sizes.iter().sum::<u16>();
                        if let Some(last) = new_sizes.last_mut() { *last += rem; }
                    }
                } else {
                    // Scale proportionally to sum to 100
                    let mut scaled: Vec<u16> = new_sizes.iter().map(|&s| (s as u32 * 100 / total as u32) as u16).collect();
                    let rem = 100u16.saturating_sub(scaled.iter().sum::<u16>());
                    if let Some(last) = scaled.last_mut() { *last += rem; }
                    new_sizes = scaled;
                }
                (Some(Node::Split { kind, sizes: new_sizes, children: new_children }), newly_dead)
            }
        }
    }
}

pub fn path_exists(node: &Node, path: &Vec<usize>) -> bool {
    let mut cur = node;
    for &idx in path.iter() {
        match cur {
            Node::Split { children, .. } => {
                if let Some(next) = children.get(idx) { cur = next; } else { return false; }
            }
            Node::Leaf(_) => return false,
        }
    }
    matches!(cur, Node::Leaf(_) | Node::Split { .. })
}

pub fn first_leaf_path(node: &Node) -> Vec<usize> {
    fn rec(n: &Node, path: &mut Vec<usize>) -> Option<Vec<usize>> {
        match n {
            Node::Leaf(_) => Some(path.clone()),
            Node::Split { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    path.push(i);
                    if let Some(p) = rec(child, path) { return Some(p); }
                    path.pop();
                }
                None
            }
        }
    }
    rec(node, &mut Vec::new()).unwrap_or_default()
}

/// Find the tree path to a pane by its ID.  Returns None if not found.
pub fn find_path_by_id(node: &Node, id: usize) -> Option<Vec<usize>> {
    fn rec(n: &Node, id: usize, path: &mut Vec<usize>) -> Option<Vec<usize>> {
        match n {
            Node::Leaf(p) => if p.id == id { Some(path.clone()) } else { None },
            Node::Split { children, .. } => {
                for (i, c) in children.iter().enumerate() {
                    path.push(i);
                    if let Some(p) = rec(c, id, path) { return Some(p); }
                    path.pop();
                }
                None
            }
        }
    }
    rec(node, id, &mut Vec::new())
}

/// Collect all leaf pane paths in DFS order.
fn collect_leaf_paths(node: &Node, path: &mut Vec<usize>, out: &mut Vec<(usize, Vec<usize>)>) {
    match node {
        Node::Leaf(p) => out.push((p.id, path.clone())),
        Node::Split { children, .. } => {
            for (i, c) in children.iter().enumerate() {
                path.push(i);
                collect_leaf_paths(c, path, out);
                path.pop();
            }
        }
    }
}

/// Return the tree path of the pane at positional index `pos` (DFS order).
pub fn path_by_position(node: &Node, pos: usize) -> Option<Vec<usize>> {
    let mut out: Vec<(usize, Vec<usize>)> = Vec::new();
    collect_leaf_paths(node, &mut Vec::new(), &mut out);
    out.get(pos).map(|(_, p)| p.clone())
}

/// Get a mutable reference to the node at `path` (following Split children).
pub fn node_at_mut<'a>(node: &'a mut Node, path: &[usize]) -> Option<&'a mut Node> {
    let mut cur = node;
    for &idx in path {
        match cur {
            Node::Split { children, .. } => { cur = children.get_mut(idx)?; }
            Node::Leaf(_) => return None,
        }
    }
    Some(cur)
}

/// Swap the two subtrees located at `a` and `b` within `root`.
/// Returns true on success.  The paths must be distinct and neither may be a
/// prefix of the other — which always holds for two distinct leaf paths.
/// The split `sizes` are untouched, so only the pane *contents* change slots.
pub fn swap_nodes(root: &mut Node, a: &[usize], b: &[usize]) -> bool {
    if a == b || a.is_empty() || b.is_empty() { return false; }
    let min = a.len().min(b.len());
    if a[..min] == b[..min] { return false; } // one path is an ancestor of the other
    let pa = match node_at_mut(root, a) { Some(n) => n as *mut Node, None => return false };
    let pb = match node_at_mut(root, b) { Some(n) => n as *mut Node, None => return false };
    if pa == pb { return false; }
    // SAFETY: `pa` and `pb` point to distinct, non-overlapping nodes (the paths
    // are distinct and neither is a prefix of the other), so the swap cannot
    // create aliasing.
    unsafe { std::ptr::swap(pa, pb); }
    true
}

/// Swap the subtree at `a` in `a_root` with the subtree at `b` in `b_root`,
/// where the two roots are the layouts of two DIFFERENT windows.
///
/// tmux's `cmd_swap_pane_exec` (cmd-swap-pane.c:123 to 148) allows `-s` and
/// `-t` to live in different windows: it splices each pane into the other
/// window's pane list and exchanges the two layout cells, so the panes trade
/// places across the window boundary. `swap_nodes` cannot express that because
/// it takes a single root, and the ancestor check it needs inside one tree is
/// meaningless between two.
///
/// An empty path is allowed here (unlike `swap_nodes`): a single pane window's
/// root IS the leaf, and swapping it with a pane in another window is exactly
/// what tmux does.
pub fn swap_nodes_across(a_root: &mut Node, a: &[usize], b_root: &mut Node, b: &[usize]) -> bool {
    let Some(pa) = node_at_mut(a_root, a) else { return false };
    let Some(pb) = node_at_mut(b_root, b) else { return false };
    std::mem::swap(pa, pb);
    true
}

#[cfg(test)]
mod swap_node_tests {
    use crate::types::{Node, LayoutKind};
    // Use empty Splits as distinguishable markers (avoids constructing a Pane).
    fn marker(n: u16) -> Node {
        Node::Split { kind: LayoutKind::Vertical, sizes: vec![n], children: vec![] }
    }
    fn sz(n: &Node) -> Vec<u16> {
        match n { Node::Split { sizes, .. } => sizes.clone(), _ => vec![] }
    }
    #[test]
    fn swap_nonsibling_positions() {
        // H[ A(10), V[ B(20), C(30) ] ] ; swap [0] <-> [1,1]
        let mut root = Node::Split { kind: LayoutKind::Horizontal, sizes: vec![1, 1], children: vec![
            marker(10),
            Node::Split { kind: LayoutKind::Vertical, sizes: vec![1, 1], children: vec![marker(20), marker(30)] },
        ]};
        assert!(super::swap_nodes(&mut root, &[0], &[1, 1]));
        if let Node::Split { children, .. } = &root {
            assert_eq!(sz(&children[0]), vec![30], "slot [0] should now hold C(30)");
            if let Node::Split { children: c2, .. } = &children[1] {
                assert_eq!(sz(&c2[0]), vec![20], "B(20) unchanged");
                assert_eq!(sz(&c2[1]), vec![10], "slot [1,1] should now hold A(10)");
            } else { panic!("expected inner split"); }
        } else { panic!("expected split"); }
    }
    #[test]
    fn swap_siblings() {
        let mut root = Node::Split { kind: LayoutKind::Horizontal, sizes: vec![1, 1], children: vec![marker(1), marker(2)] };
        assert!(super::swap_nodes(&mut root, &[0], &[1]));
        if let Node::Split { children, .. } = &root {
            assert_eq!(sz(&children[0]), vec![2]);
            assert_eq!(sz(&children[1]), vec![1]);
        } else { panic!(); }
    }
    #[test]
    fn swap_rejects_invalid() {
        let mut root = Node::Split { kind: LayoutKind::Horizontal, sizes: vec![1], children: vec![marker(1)] };
        assert!(!super::swap_nodes(&mut root, &[0], &[0]));    // identical paths
        assert!(!super::swap_nodes(&mut root, &[], &[0]));     // empty path
        assert!(!super::swap_nodes(&mut root, &[0], &[0, 0])); // ancestor/descendant
    }
}

/// Public wrapper for collect_leaf_paths (used by join-pane to resolve pane index to path).
pub fn collect_leaf_paths_pub(node: &Node, path: &mut Vec<usize>, out: &mut Vec<(usize, Vec<usize>)>) {
    collect_leaf_paths(node, path, out);
}

/// Move `pane_id` to the front of the MRU list.
/// If not present, inserts at front.
pub fn touch_mru(mru: &mut Vec<usize>, pane_id: usize) {
    if let Some(pos) = mru.iter().position(|&id| id == pane_id) {
        mru.remove(pos);
    }
    mru.insert(0, pane_id);
}

/// Remove a pane ID from the MRU list.
pub fn remove_from_mru(mru: &mut Vec<usize>, pane_id: usize) {
    mru.retain(|&id| id != pane_id);
}

/// Get the MRU rank of a pane ID (0 = most recent). Returns usize::MAX if not found.
pub fn mru_rank(mru: &[usize], pane_id: usize) -> usize {
    mru.iter().position(|&id| id == pane_id).unwrap_or(usize::MAX)
}

/// Visit every pane in a tree node (DFS order), calling `f` on each.
pub fn for_each_pane(node: &Node, f: &mut dyn FnMut(&Pane)) {
    match node {
        Node::Leaf(p) => f(p),
        Node::Split { children, .. } => {
            for c in children { for_each_pane(c, f); }
        }
    }
}

/// Visit every pane in a tree node (DFS order), calling `f` on each, mutably.
pub fn for_each_pane_mut(node: &mut Node, f: &mut dyn FnMut(&mut Pane)) {
    match node {
        Node::Leaf(p) => f(p),
        Node::Split { children, .. } => {
            for c in children { for_each_pane_mut(c, f); }
        }
    }
}

/// Collect all pane IDs from a tree node (DFS order).
pub fn collect_pane_ids(node: &Node) -> Vec<usize> {
    let mut ids = Vec::new();
    fn rec(node: &Node, ids: &mut Vec<usize>) {
        match node {
            Node::Leaf(p) => ids.push(p.id),
            Node::Split { children, .. } => {
                for c in children { rec(c, ids); }
            }
        }
    }
    rec(node, &mut ids);
    ids
}

/// Find the next pane path after `active_path` in DFS order (wraps around).
/// Returns the path of the next pane, or None if there's only one pane.
pub fn next_leaf_path(node: &Node, active_path: &[usize]) -> Option<Vec<usize>> {
    let mut leaves = Vec::new();
    collect_leaf_paths(node, &mut Vec::new(), &mut leaves);
    if leaves.len() <= 1 { return None; }
    let pos = leaves.iter().position(|(_, p)| p.as_slice() == active_path).unwrap_or(0);
    let next = if pos + 1 < leaves.len() { pos + 1 } else { pos.saturating_sub(1) };
    Some(leaves[next].1.clone())
}

/// Get the pane ID of the active pane
pub fn get_active_pane_id(node: &Node, path: &[usize]) -> Option<usize> {
    match node {
        Node::Leaf(p) => Some(p.id),
        Node::Split { children, .. } => {
            if let Some(&idx) = path.first() {
                if let Some(child) = children.get(idx) {
                    return get_active_pane_id(child, &path[1..]);
                }
            }
            children.first().and_then(|c| get_active_pane_id(c, &[]))
        }
    }
}

/// Get the pane ID at a specific path (used by format vars for pane position lookup).
pub fn get_active_pane_id_at_path(node: &Node, path: &[usize]) -> Option<usize> {
    get_active_pane_id(node, path)
}

/// Get the positional index (0-based) of a pane within its window, by pane ID.
/// Panes are enumerated in tree traversal order (left-to-right, top-to-bottom).
pub fn get_pane_position_in_window(node: &Node, target_id: usize) -> Option<usize> {
    fn collect_ids(node: &Node, ids: &mut Vec<usize>) {
        match node {
            Node::Leaf(p) => ids.push(p.id),
            Node::Split { children, .. } => {
                for c in children { collect_ids(c, ids); }
            }
        }
    }
    let mut ids = Vec::new();
    collect_ids(node, &mut ids);
    ids.iter().position(|&id| id == target_id)
}

/// Locate a pane by its global pane ID across every window in the app.
/// Returns (window_index, pane_position_within_window) or None if no pane
/// with that id exists. Used to make bare `%N` -t targets work for any
/// command that operates on pane *position* internally (issue #332).
pub fn find_pane_by_id_global(app: &AppState, pane_id: usize) -> Option<(usize, usize)> {
    for (wi, w) in app.windows.iter().enumerate() {
        if let Some(pos) = get_pane_position_in_window(&w.root, pane_id) {
            return Some((wi, pos));
        }
    }
    None
}

/// Mutable access to a pane by its global pane ID across every window
/// (issue #580: pane-scoped options must resolve `%N` regardless of the
/// active window, like every other bare-%id target).
pub fn find_pane_mut_by_id_global(app: &mut AppState, pane_id: usize) -> Option<&mut Pane> {
    fn walk(node: &mut Node, pane_id: usize) -> Option<&mut Pane> {
        match node {
            Node::Leaf(p) => if p.id == pane_id { Some(p) } else { None },
            Node::Split { children, .. } => {
                for c in children {
                    if let Some(p) = walk(c, pane_id) { return Some(p); }
                }
                None
            }
        }
    }
    for w in app.windows.iter_mut() {
        if let Some(p) = walk(&mut w.root, pane_id) { return Some(p); }
    }
    None
}

/// Get the Nth leaf pane (0-based positional index) from the tree.
pub fn get_nth_pane(node: &Node, n: usize) -> Option<&Pane> {
    fn collect_panes<'a>(node: &'a Node, panes: &mut Vec<&'a Pane>) {
        match node {
            Node::Leaf(p) => panes.push(p),
            Node::Split { children, .. } => {
                for c in children { collect_panes(c, panes); }
            }
        }
    }
    let mut panes = Vec::new();
    collect_panes(node, &mut panes);
    panes.get(n).copied()
}

pub fn find_window_index_by_id(app: &AppState, wid: usize) -> Option<usize> {
    app.windows.iter().position(|w| w.id == wid)
}

pub fn focus_pane_by_id(app: &mut AppState, pid: usize) {
    focus_pane_by_id_inner(app, pid, true);
}

/// Like `focus_pane_by_id` but does NOT update MRU.
/// Used for temporary -t targeting where the focus change is transient
/// and should not pollute the recency list (#71).
pub fn focus_pane_by_id_no_mru(app: &mut AppState, pid: usize) {
    focus_pane_by_id_inner(app, pid, false);
}

fn focus_pane_by_id_inner(app: &mut AppState, pid: usize, update_mru: bool) {
    fn rec(node: &Node, path: &mut Vec<usize>, found: &mut Option<Vec<usize>>, pid: usize) {
        match node {
            Node::Leaf(p) => { if p.id == pid { *found = Some(path.clone()); } }
            Node::Split { children, .. } => {
                for (i, c) in children.iter().enumerate() { path.push(i); rec(c, path, found, pid); path.pop(); if found.is_some() { return; } }
            }
        }
    }
    for (wi, w) in app.windows.iter().enumerate() {
        let mut path = Vec::new();
        let mut found = None;
        rec(&w.root, &mut path, &mut found, pid);
        if let Some(p) = found {
            app.active_idx = wi;
            let win = &mut app.windows[wi];
            win.active_path = p;
            // A tiled pane taking the focus takes it from a focused floating
            // pane too: input follows `floating_focus` first, so leaving it
            // set kept `select-pane -t %1` and every `-t %1` request on the
            // float (issue #793 follow up).
            win.floating_focus = None;
            if update_mru { touch_mru(&mut win.pane_mru, pid); }
            return;
        }
    }
    // A floating pane (tmux new-pane): focusing it is giving it the input
    // focus in its window. Before, a floating %id matched nothing here and
    // the request quietly ran on whatever was focused.
    for (wi, w) in app.windows.iter_mut().enumerate() {
        if let Some(fi) = w.floating.iter().position(|fp| fp.id == pid) {
            w.floating_focus = Some(fi);
            app.active_idx = wi;
            return;
        }
    }
}

/// The window and index of floating pane `pid`, if it is one.
pub fn find_floating_by_id(app: &AppState, pid: usize) -> Option<(usize, usize)> {
    app.windows.iter().enumerate().find_map(|(wi, w)| {
        w.floating.iter().position(|fp| fp.id == pid).map(|fi| (wi, fi))
    })
}

pub fn focus_pane_by_index(app: &mut AppState, idx: usize) {
    fn collect_pane_paths(node: &Node, path: &mut Vec<usize>, panes: &mut Vec<Vec<usize>>) {
        match node {
            Node::Leaf(_) => { panes.push(path.clone()); }
            Node::Split { children, .. } => {
                for (i, c) in children.iter().enumerate() {
                    path.push(i);
                    collect_pane_paths(c, path, panes);
                    path.pop();
                }
            }
        }
    }
    let win = &mut app.windows[app.active_idx];
    let mut pane_paths = Vec::new();
    let mut path = Vec::new();
    collect_pane_paths(&win.root, &mut path, &mut pane_paths);
    if let Some(path) = pane_paths.get(idx) {
        win.active_path = path.clone();
    }
}

/// Every pane path in a window's tree, in pane-index order.
pub fn pane_paths(node: &Node) -> Vec<Vec<usize>> {
    fn rec(node: &Node, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        match node {
            Node::Leaf(_) => out.push(path.clone()),
            Node::Split { children, .. } => {
                for (i, c) in children.iter().enumerate() {
                    path.push(i);
                    rec(c, path, out);
                    path.pop();
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut path = Vec::new();
    rec(node, &mut path, &mut out);
    out
}

/// Vec position of the window that holds pane `%pid`.
pub fn find_window_pos_of_pane_id(app: &AppState, pid: usize) -> Option<usize> {
    app.windows.iter().position(|w| {
        pane_paths(&w.root).into_iter().any(|p| get_active_pane_id(&w.root, &p) == Some(pid))
    })
}

/// Make pane index `idx` the active pane OF THE WINDOW at `win_pos`, without
/// making that window the session's current window.
///
/// This is tmux's `window_set_active_pane(w, wp, 1)`, which cmd-select-pane.c
/// calls on the TARGET window (`wl->window`, :274) and never follows with a
/// `session_select`. psmux used to move the current window first, so
/// `select-pane -t s:1.0` switched the user's window as a side effect
/// (issue #693 item 3).
///
/// Returns true when the window's active pane actually moved.
pub fn set_window_active_pane_by_index(app: &mut AppState, win_pos: usize, idx: usize) -> bool {
    let Some(win) = app.windows.get_mut(win_pos) else { return false };
    let paths = pane_paths(&win.root);
    let Some(path) = paths.get(idx) else { return false };
    if win.active_path == *path { return false; }
    win.active_path = path.clone();
    if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
        touch_mru(&mut win.pane_mru, pid);
    }
    true
}

/// `set_window_active_pane_by_index` for a `%id` target.
pub fn set_window_active_pane_by_id(app: &mut AppState, win_pos: usize, pid: usize) -> bool {
    let Some(win) = app.windows.get_mut(win_pos) else { return false };
    let paths = pane_paths(&win.root);
    let target = paths.into_iter().find(|p| {
        get_active_pane_id(&win.root, p) == Some(pid)
    });
    let Some(path) = target else { return false };
    if win.active_path == path { return false; }
    win.active_path = path;
    touch_mru(&mut win.pane_mru, pid);
    true
}

/// Count the number of leaf (pane) nodes in a tree.
pub fn count_panes(node: &Node) -> usize {
    match node {
        Node::Leaf(_) => 1,
        Node::Split { children, .. } => children.iter().map(count_panes).sum(),
    }
}

/// Immutable reference to the active pane (follows path through splits).
pub fn active_pane<'a>(node: &'a Node, path: &[usize]) -> Option<&'a Pane> {
    match node {
        Node::Leaf(p) => Some(p),
        Node::Split { children, .. } => {
            if path.is_empty() { return None; }
            let idx = path[0].min(children.len().saturating_sub(1));
            active_pane(&children[idx], &path[1..])
        }
    }
}

/// Get the index of the pane at `path` among all leaf panes in the window tree (DFS order).
pub fn pane_index_in_window(node: &Node, path: &[usize]) -> Option<usize> {
    // Find the pane ID at the path, then count its position
    let target = active_pane(node, path)?;
    let target_id = target.id;
    let mut idx = 0usize;
    fn walk(n: &Node, target_id: usize, idx: &mut usize) -> bool {
        match n {
            Node::Leaf(p) => {
                if p.id == target_id { return true; }
                *idx += 1;
                false
            }
            Node::Split { children, .. } => {
                for c in children {
                    if walk(c, target_id, idx) { return true; }
                }
                false
            }
        }
    }
    if walk(node, target_id, &mut idx) { Some(idx) } else { None }
}

/// Reap exited children from the app.
/// Returns `(all_empty, any_pruned, any_newly_dead)`:
/// - `any_pruned`: at least one pane was removed from the tree (remain-on-exit off)
/// - `any_newly_dead`: at least one pane transitioned alive→dead (remain-on-exit on)
///
/// Callers should fire pane-died/pane-exited hooks when either flag is true,
/// and only resize the layout when `any_pruned` is true.
///
/// Fast check: does any pane in this node tree have an exited child?
/// Uses try_wait() but avoids the full tree rebuild if nothing has exited.
fn has_any_exited(node: &mut Node) -> bool {
    match node {
        Node::Leaf(p) => {
            if p.dead { return false; } // Already dead, handled
            matches!(p.child.try_wait(), Ok(Some(_)))
        }
        Node::Split { children, .. } => {
            children.iter_mut().any(|c| has_any_exited(c))
        }
    }
}

pub fn reap_children(app: &mut AppState) -> io::Result<(bool, bool, bool)> {
    let global_remain = app.remain_on_exit;
    let kill_descendants = app.kill_descendants_on_exit();
    let mut any_pruned = false;
    let mut any_newly_dead = false;
    for i in (0..app.windows.len()).rev() {
        // Fast path: skip full tree rebuild if no panes have exited
        if !has_any_exited(&mut app.windows[i].root) {
            continue;
        }
        let leaves_before = count_panes(&app.windows[i].root);
        let active_pane_id = get_active_pane_id(&app.windows[i].root, &app.windows[i].active_path);
        // #648: remain-on-exit is a WINDOW option. The reaper used to read one
        // session-wide flag for every window, so `set -w -t <one window>
        // remain-on-exit on` kept dead panes alive everywhere; the reporter's
        // ordinary PowerShell panes stopped closing after `exit` because of it.
        let remain = crate::server::options::window_flag(app, i, "remain-on-exit", global_remain);
        normalize_window_sizes(&mut app.windows[i]);
        let root = std::mem::replace(&mut app.windows[i].root, Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] });
        let (pruned_result, newly_dead_count) = prune_exited(root, remain, kill_descendants);
        if newly_dead_count > 0 {
            any_newly_dead = true;
        }
        match pruned_result {
            Some(new_root) => {
                let leaves_after = count_panes(&new_root);
                if leaves_after < leaves_before {
                    any_pruned = true;
                    // Clean up MRU: remove IDs of panes that no longer exist
                    let surviving_ids = collect_pane_ids(&new_root);
                    app.windows[i].pane_mru.retain(|id| surviving_ids.contains(id));
                }
                app.windows[i].root = new_root;
                // After tree restructuring, the old active_path indices may
                // still be in-range but point to a different pane (issue #140).
                // Always verify by pane ID, not just path validity.
                let current_id = get_active_pane_id(&app.windows[i].root, &app.windows[i].active_path);
                if current_id != active_pane_id || !path_exists(&app.windows[i].root, &app.windows[i].active_path) {
                    // The active pane's path shifted due to tree restructuring.
                    // Try to find it by ID first, then by MRU order (issue #71).
                    let found = active_pane_id.and_then(|id| find_path_by_id(&app.windows[i].root, id))
                        .or_else(|| {
                            app.windows[i].pane_mru.iter()
                                .find_map(|&id| find_path_by_id(&app.windows[i].root, id))
                        });
                    app.windows[i].active_path = found.unwrap_or_else(|| first_leaf_path(&app.windows[i].root));
                }
            }
            None => {
                app.windows.remove(i);
                app.on_window_removed(i);
                any_pruned = true;
                // Adjust active_idx after removing a window
                let _old = app.active_idx;
                if !app.windows.is_empty() {
                    if i < app.active_idx {
                        app.active_idx -= 1;
                    } else if app.active_idx >= app.windows.len() {
                        app.active_idx = app.windows.len() - 1;
                    }
                }
                if app.active_idx != _old {
                    crate::debug_log::server_log("switch", &format!(
                        "REAP: active_idx {} -> {} after removing window at index {}", _old, app.active_idx, i));
                }
            }
        }
    }
    Ok((app.windows.is_empty(), any_pruned, any_newly_dead))
}

/// Collect all leaf (Pane) nodes from the tree, consuming it.
/// Returns them in DFS (left-to-right) order.
pub fn collect_leaves(node: Node) -> Vec<Node> {
    match node {
        Node::Leaf(_) => vec![node],
        Node::Split { children, .. } => {
            let mut leaves = Vec::new();
            for child in children {
                leaves.extend(collect_leaves(child));
            }
            leaves
        }
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_issue171_layout_bugs.rs"]
mod test_issue171_layout_bugs;

#[cfg(test)]
#[path = "../tests-rs/test_issue644_one_row_pane.rs"]
mod test_issue644_one_row_pane;

#[cfg(test)]
#[path = "../tests-rs/test_split_weight_drag.rs"]
mod test_split_weight_drag;
