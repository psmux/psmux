p = r'C:\Users\godwin\Documents\workspace\psmux\.claude\worktrees\agent-ad44d5289fdea1c2f\src\client.rs'
s = open(p, 'rb').read().decode('utf-8')


def rep(old, new, count=1):
    global s
    old = old.replace('\n', '\r\n')
    new = new.replace('\n', '\r\n')
    assert s.count(old) == count, (s.count(old), old[:80])
    s = s.replace(old, new)


rep('''    let mut tree_all: Vec<(bool, usize, usize, String, String)> = Vec::new();
''', '''    let mut tree_all: Vec<(bool, usize, usize, String, String)> = Vec::new();
    // (session base, window id) -> window index, for the `=s:idx.` target a
    // choose-tree template is given (tmux window_tree_get_target).
    let mut tree_win_index: std::collections::HashMap<(String, usize), usize> = std::collections::HashMap::new();
    // choose-tree's template argument (#912): run for the chosen row in
    // place of the chooser's own switch. None for prefix w / prefix s.
    let mut chooser_template: Option<String> = None;
''')
rep('''            tree_collapsed.clear();
            tree_vis_map.clear();
            tree_selected = 0;
            tree_scroll = 0;
''', '''            tree_collapsed.clear();
            tree_vis_map.clear();
            tree_win_index.clear();
            tree_selected = 0;
            tree_scroll = 0;
''')
rep('''                    for (wid, wname, active, panes, disp) in wins.iter() {
                        let marker = if *active { "*" } else { "" };
''', '''                    for (wid, wname, active, panes, disp) in wins.iter() {
                        tree_win_index.insert((sess_name.clone(), *wid), *disp);
                        let marker = if *active { "*" } else { "" };
''')
rep('''        let mut pending_chooser: Option<crate::choose_tree::ChooserKind> = None;''',
    '''        let mut pending_chooser: Option<(crate::choose_tree::ChooserKind, Option<String>)> = None;''')
rep('''                    } else if let Some(kind) = crate::choose_tree::ChooserKind::from_directive(line.trim()) {''',
    '''                    } else if let Some(open) = crate::choose_tree::parse_directive(line.trim()) {''')
rep('''                        pending_chooser = Some(kind);''', '''                        pending_chooser = Some(open);''')
rep('''        match pending_chooser {
            Some(crate::choose_tree::ChooserKind::Tree) => {
                session_chooser = false;
                buffer_chooser = false;
                open_tree_chooser!();
            }
            Some(crate::choose_tree::ChooserKind::Session) => {
                tree_chooser = false;
                buffer_chooser = false;
                open_session_chooser!();
            }
            None => {}
        }''', '''        match pending_chooser {
            Some((crate::choose_tree::ChooserKind::Tree, template)) => {
                session_chooser = false;
                buffer_chooser = false;
                open_tree_chooser!();
                chooser_template = template;
            }
            Some((crate::choose_tree::ChooserKind::Session, template)) => {
                tree_chooser = false;
                buffer_chooser = false;
                open_session_chooser!();
                chooser_template = template;
            }
            None => {}
        }''')
rep('''                            if do_choose_tree {
                                open_tree_chooser!();
                            }
                            if do_choose_session {
                                open_session_chooser!();''', '''                            if do_choose_tree {
                                open_tree_chooser!();
                                chooser_template = None;
                            }
                            if do_choose_session {
                                open_session_chooser!();
                                chooser_template = None;''')
rep('''                                    selection_changed = true;
                                    if let Some((is_win, wid, pid, _label, sess_name)) = tree_entries.get(tree_selected) {
                                        if *wid == usize::MAX {''', '''                                    selection_changed = true;
                                    if let Some(template) = chooser_template.take() {
                                        // choose-tree [template] (#912): tmux puts the
                                        // row's target where the template says and runs
                                        // that instead (window_tree_key '\\r',
                                        // mode_tree_run_command). No switch of its own.
                                        if let Some((is_win, wid, pid, _label, sess_name)) = tree_entries.get(tree_selected) {
                                            let ns = crate::session::session_namespace(&current_session);
                                            let name = crate::session::display_session_name(ns, sess_name);
                                            let target = if *wid == usize::MAX {
                                                crate::choose_tree::session_row_target(name)
                                            } else {
                                                match tree_win_index.get(&(sess_name.clone(), *wid)) {
                                                    Some(&idx) if *is_win => crate::choose_tree::window_row_target(name, idx),
                                                    Some(&idx) => crate::choose_tree::pane_row_target(name, idx, *pid),
                                                    None if *is_win => format!("@{}", wid),
                                                    None => format!("%{}", pid),
                                                }
                                            };
                                            let command = crate::choose_tree::template_command(&template, &target);
                                            if !command.trim().is_empty() {
                                                let sub_cmds = crate::config::split_chained_commands_pub(&command);
                                                cmd_batch.extend(binding_wire_lines(&sub_cmds));
                                            }
                                        }
                                        tree_chooser = false;
                                    } else if let Some((is_win, wid, pid, _label, sess_name)) = tree_entries.get(tree_selected) {
                                        if *wid == usize::MAX {''')
rep('''                                        if let Some((sname, _)) = session_entries.get(idx) {
                                            if sname != &current_session {
                                                cmd_batch.push("client-detach\\n".into());
                                                env::set_var("PSMUX_SWITCH_TO", sname);
                                                quit = true;
                                            }''', '''                                        if let Some((sname, _)) = session_entries.get(idx) {
                                            if let Some(template) = chooser_template.take() {
                                                // choose-tree -s [template] (#912): the
                                                // session row's target `=name:` goes in.
                                                let ns = crate::session::session_namespace(&current_session);
                                                let target = crate::choose_tree::session_row_target(
                                                    crate::session::display_session_name(ns, sname));
                                                let command = crate::choose_tree::template_command(&template, &target);
                                                if !command.trim().is_empty() {
                                                    let sub_cmds = crate::config::split_chained_commands_pub(&command);
                                                    cmd_batch.extend(binding_wire_lines(&sub_cmds));
                                                }
                                            } else if sname != &current_session {
                                                cmd_batch.push("client-detach\\n".into());
                                                env::set_var("PSMUX_SWITCH_TO", sname);
                                                quit = true;
                                            }''')
open(p, 'wb').write(s.encode('utf-8'))
print("ok")
