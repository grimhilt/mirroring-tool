use std::env;
use std::process::{Command, Stdio};
use swayipc::{Connection, Event, EventType, Fallible, WorkspaceChange, OutputChange};
use serde_json::Value;

const DEFAULT_MIRROR_WS: &str = "5";

fn move_to_workspace(name: &str, connection: &mut Connection) -> Fallible<()> {
    connection.run_command(format!("workspace number {}", name))?;
    Ok(())
}

struct WorkspaceHistory {
    prev: Option<String>,
    skip_next: bool,
}

impl WorkspaceHistory {
    fn new() -> Self {
        Self { prev: None, skip_next: false }
    }

    fn should_consider(&mut self) -> bool {
        if self.skip_next {
            self.skip_next = false;
            false
        } else {
            true
        }
    }

    fn redirect_from_prev(&self, connection: &mut Connection) -> Fallible<()> {
        if let Some(prev) = &self.prev {
            move_to_workspace(prev, connection)?;
        }
        Ok(())
    }

    fn redirect_from_mirror(&mut self, connection: &mut Connection) -> Fallible<()> {
        let outputs = connection.get_outputs()?;
        if outputs.len() == 1 {
            return Ok(());
        }

        if let Some(output) = outputs.iter().find(|o| !o.focused) {
            if let Some(ws) = &output.current_workspace {
                self.skip_next = true;
                move_to_workspace(ws, connection)?;
            }
        }
        Ok(())
    }
}

/// Launch wl-mirror when a new output is detected.
fn launch_wl_mirror(args: &[String]) -> Fallible<()> {
    println!("[INFO] Launching wl-mirror with args: {:?}", args);
    let mut cmd = Command::new("wl-mirror");
    cmd.args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.into())
}

fn main() -> Fallible<()> {
    let args: Vec<String> = env::args().collect();

    // Extract mirror workspace number
    let mirror_ws = args.get(1)
        .cloned()
        .unwrap_or_else(|| DEFAULT_MIRROR_WS.to_string());

    // Everything after the `--` will be passed to wl-mirror
    let mirror_args: Vec<String> = if let Some(idx) = args.iter().position(|x| x == "--") {
        args[(idx + 1)..].to_vec()
    } else {
        Vec::new()
    };

    println!("[INFO] Using mirror workspace {}", mirror_ws);
    if !mirror_args.is_empty() {
        println!("[INFO] wl-mirror will be launched with: {:?}", mirror_args);
    }

    let mut connection = Connection::new()?;
    let mut history = WorkspaceHistory::new();

    // Subscribe to both workspace and output events
    let subs = [EventType::Workspace, EventType::Output];

    for event in Connection::new()?.subscribe(subs)? {
        match event? {
            // Handle workspace focus changes
            Event::Workspace(w) if w.change == WorkspaceChange::Focus => {
                if !history.should_consider() {
                    continue;
                }

                let current = match w.current {
                    Some(c) => c,
                    None => continue,
                };
                let current_name = current.name.unwrap_or_default();

                if current_name == mirror_ws {
                    if history.prev.is_some() {
                        history.redirect_from_prev(&mut connection)?;
                        history.prev = None;
                    } else if let Some(old) = w.old.and_then(|o| o.name) {
                        history.redirect_from_mirror(&mut connection)?;
                        history.prev = Some(old);
                    }
                } else if let Some(_prev) = &history.prev {
                    let ws_output_current = connection.get_workspaces()?.into_iter()
                        .find(|w| w.name == current_name)
                        .map(|w| w.output);
                    let ws_output_mirror = connection.get_workspaces()?.into_iter()
                        .find(|w| w.name == mirror_ws)
                        .map(|w| w.output);

                    if ws_output_current == ws_output_mirror {
                        history.skip_next = true;
                        move_to_workspace(&mirror_ws, &mut connection)?;
                        move_to_workspace(&current_name, &mut connection)?;
                        history.prev = None;
                    }
                }
            }

            // Handle output (screen) events
            Event::Output(raw_event) => {
            let json = serde_json::to_value(&raw_event).unwrap_or_default();

            if let Some(change) = json.get("change").and_then(|c| c.as_str()) {
                if change == "added" {
                    println!("[INFO] New output detected: {:?}", json);
                    if let Err(e) = launch_wl_mirror(&mirror_args) {
                        eprintln!("[ERROR] Failed to start wl-mirror: {}", e);
                    }
                }
            }
        }

            _ => {}
        }
    }

    Ok(())
}
