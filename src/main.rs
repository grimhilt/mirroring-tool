use std::env;
use std::process::{Command, Stdio};
use std::collections::HashSet;
use swayipc::{Connection, Event, EventType, Fallible, WorkspaceChange, OutputChange, OutputEvent};
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

fn launch_wl_mirror(output_name: String, args: &[String]) -> Fallible<()> {
    println!("[INFO] Launching wl-mirror on '{}' with args: {:?}", output_name, args);

    let mut cmd = std::process::Command::new("wl-mirror");
    cmd.arg(&output_name)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
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

    // Everything after `--` goes to wl-mirror
    let mirror_args: Vec<String> = if let Some(idx) = args.iter().position(|x| x == "--") {
        args[(idx + 1)..].to_vec()
    } else {
        Vec::new()
    };

    let mut activate = false;

    println!("[INFO] Using mirror workspace {}", mirror_ws);
    if !mirror_args.is_empty() {
        println!("[INFO] wl-mirror will be launched with: {:?}", mirror_args);
    }

    let mut connection = Connection::new()?;
    let mut history = WorkspaceHistory::new();
    let mut mirrored_outputs: HashSet<String> = HashSet::new();

    let subs = [EventType::Workspace, EventType::Output];

    for event in Connection::new()?.subscribe(subs)? {
        match event? {
            // Workspace focus handling (same as before)
            Event::Workspace(w) if w.change == WorkspaceChange::Focus => {
                if !activate {
                    continue
                }

                // Count active outputs
                let outs = connection.get_outputs()?;
                let active_outputs: Vec<_> = outs.iter().filter(|o| o.active).collect();

                // If only one monitor, skip mirroring behavior
                if active_outputs.len() < 2 {
                    println!("[INFO] Only one active output detected, skipping mirror behavior.");
                    mirrored_outputs.clear(); // reset so future plug-ins can trigger
                    continue;
                }

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

            Event::Output(OutputEvent { change: OutputChange::Unspecified, .. }) => {
                // Query all outputs
                let outs = connection.get_outputs()?;
                if outs.is_empty() {
                    continue;
                }

                // Determine primary output (focused one)
                let primary_output = outs.iter()
                    .find(|o| o.focused)
                    .map(|o| o.name.clone())
                    .unwrap_or_else(|| outs[0].name.clone());

                for o in &outs {
                    let name = &o.name;

                    // Skip the primary output
                    if name == &primary_output {
                        continue;
                    }

                    // Skip inactive outputs
                    if !o.active {
                        continue;
                    }

                    // Skip if already mirrored
                    if mirrored_outputs.contains(name) {
                        continue;
                    }

                    println!("[INFO] Detected new secondary active output: {}", name);
                    mirrored_outputs.insert(name.clone());

                    // Switch to mirror workspace
                    if let Err(e) = move_to_workspace(&mirror_ws, &mut connection) {
                        eprintln!("[WARN] Failed to switch to mirror workspace: {}", e);
                    }

                    // Launch wl-mirror on the secondary screen
                    if let Err(e) = launch_wl_mirror(name.clone(), &mirror_args) {
                        eprintln!("[ERROR] Failed to start wl-mirror for {}: {}", name, e);
                    }
                    // Activate after launching wl_mirror
                    activate = true;

                    let current_ws = connection.get_workspaces()?
                        .into_iter()
                        .find(|w| w.focused)
                        .and_then(|w| w.name.parse::<String>().ok())
                        .unwrap_or_else(|| "1".to_string());

                    // Optional: switch back to previous workspace
                    if let Err(e) = move_to_workspace(&current_ws, &mut connection) {
                        eprintln!("[WARN] Failed to return to previous workspace: {}", e);
                    }

                }
            }

            _ => {}
        }
    }

    Ok(())
}
