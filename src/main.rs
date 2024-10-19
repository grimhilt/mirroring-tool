use std::collections::HashMap;
use std::env;
use swayipc::{Connection, Event, EventType, Fallible, WorkspaceChange};

const DEFAULT_MIRROR_WS: &str = "5";

fn move_to_workspace(name: &String, connection: &mut Connection) -> Fallible<()> {
    connection.run_command(format!("workspace number {}", name))?;
    Ok(())
}

struct WorkspaceHistory {
    prev: Option<String>,
    // At true if next focus event will be trigger by switching to mirror WS
    skip: u8,
}

impl WorkspaceHistory {
    fn new() -> Self {
        WorkspaceHistory {
            prev: None,
            skip: 0,
        }
    }

    fn redirect_from_prev(&self, connection: &mut Connection) -> Fallible<()> {
        move_to_workspace(&self.prev.clone().unwrap(), connection)?;
        Ok(())
    }

    fn redirect_from_mirror(&mut self, connection: &mut Connection) -> Fallible<()> {
        let outputs = connection.get_outputs()?;
        // Cancel as there is no secondary monitor
        if outputs.len() == 1 {
            return Ok(());
        }
        let current_workspace = outputs
            .iter()
            .last()
            .unwrap()
            .current_workspace
            .clone()
            .unwrap();

        self.skip += 1;
        move_to_workspace(&current_workspace, connection)?;
        Ok(())
    }

    fn should_consider(&mut self) -> bool {
        if self.skip > 0 {
            self.skip -= 1;
            return false;
        }
        return true;
    }
}

fn main() -> Fallible<()> {
    let args: Vec<String> = env::args().collect();
    let mirror_ws: String = if args.len() > 1 {
        args[1].to_string()
    } else {
        DEFAULT_MIRROR_WS.to_string()
    };

    let mut connection = swayipc::Connection::new()?;
    let ws_output: HashMap<String, String> = connection
        .get_workspaces()?
        .into_iter()
        .map(|w| (w.name, w.output))
        .collect();

    let subs = [EventType::Workspace];
    let mut history = WorkspaceHistory::new();

    // Event loop
    for event in Connection::new()?.subscribe(subs)? {
        match event? {
            Event::Workspace(w) if w.change == WorkspaceChange::Focus => {
                // Skip events triggered by us
                if !history.should_consider() {
                    continue;
                }

                let current = w.current.unwrap();
                let current_name = current.name.unwrap();

                if current_name == mirror_ws {
                    if history.prev.is_some() {
                        // On mirror_ws from back_and_forth then redirect
                        // to workspace before being on mirror_ws
                        history.redirect_from_prev(&mut connection)?;
                        history.prev = None;
                    } else {
                        // Redirect to secondary monitor
                        history.redirect_from_mirror(&mut connection)?;
                        history.prev = Some(w.old.unwrap().name.unwrap());
                    }
                } else if history.prev.is_some() {
                    if ws_output.get(&current_name) == ws_output.get(&mirror_ws) {
                        // if directly going back to a workspace on main monitor
                        // and was previously on mirror_ws then pass through
                        // the mirror_ws for history
                        history.skip = 2;
                        move_to_workspace(&mirror_ws, &mut connection)?;
                        move_to_workspace(&current_name, &mut connection)?;
                        history.prev = None;
                    }
                }
            }
            _ => (),
        }
    }
    Ok(())
}
