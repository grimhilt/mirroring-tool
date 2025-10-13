use std::collections::HashSet;
use std::env;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use swayipc::{Connection, Event, EventType, Fallible, OutputChange, OutputEvent, WorkspaceChange};

const DEFAULT_MIRROR_WS: &str = "5";

struct WorkspaceHistory {
    prev: Option<String>,
    skip_next: bool,
}

impl WorkspaceHistory {
    fn new() -> Self {
        Self {
            prev: None,
            skip_next: false,
        }
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
            MirrorManager::move_to_workspace(prev, connection)?;
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
                MirrorManager::move_to_workspace(ws, connection)?;
            }
        }
        Ok(())
    }
}

struct MirrorManager {
    mirror_ws: String,
    mirror_args: Vec<String>,
    connection: Connection,
    history: WorkspaceHistory,
    mirrored_outputs: HashSet<String>,
    active: bool,
}

impl MirrorManager {
    fn new(mirror_ws: String, mirror_args: Vec<String>) -> Fallible<Self> {
        Ok(Self {
            mirror_ws,
            mirror_args,
            connection: Connection::new()?,
            history: WorkspaceHistory::new(),
            mirrored_outputs: HashSet::new(),
            active: false,
        })
    }

    fn move_to_workspace(name: &str, connection: &mut Connection) -> Fallible<()> {
        connection.run_command(format!("workspace number {}", name))?;
        Ok(())
    }

    fn launch_wl_mirror(&self, output_name: &str) -> Fallible<()> {
        println!(
            "[INFO] Launching wl-mirror on '{}' with args: {:?}",
            output_name, self.mirror_args
        );
        Command::new("wl-mirror")
            .args(&self.mirror_args)
            .arg(output_name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        thread::sleep(Duration::from_millis(100));
        Ok(())
    }

    fn find_output_by_name(&mut self, name: &str) -> Result<Option<String>, swayipc::Error> {
        Ok(self
            .connection
            .get_workspaces()?
            .into_iter()
            .find(|w| w.name == name)
            .map(|w| w.output))
    }

    fn get_focused_workspace(&mut self) -> Result<String, swayipc::Error> {
        Ok(self
            .connection
            .get_workspaces()?
            .into_iter()
            .find(|w| w.focused)
            .and_then(|w| w.name.parse::<String>().ok())
            .unwrap_or_else(|| "1".to_string()))
    }

    fn handle_workspace_event(&mut self, w_event: swayipc::WorkspaceEvent) -> Fallible<()> {
        if !self.active {
            return Ok(());
        }

        let outs = self.connection.get_outputs()?;
        let active_outputs: Vec<_> = outs.iter().filter(|o| o.active).collect();
        if active_outputs.len() < 2 {
            println!("[INFO] Only one active output detected, disabling moving behavior.");
            self.active = false;
            self.mirrored_outputs.clear();
            return Ok(());
        }

        // If the event should be ignored (e.g., redirect just happened)
        if !self.history.should_consider() {
            return Ok(());
        }

        let current = match w_event.current {
            Some(c) => c,
            None => return Ok(()),
        };
        let current_name = current.name.unwrap_or_default();

        if current_name == self.mirror_ws {
            if self.history.prev.is_some() {
                // Case 1: user goes back to mirror after switching from real
                // workspace -> redirect them back to previous workspace
                self.history.redirect_from_prev(&mut self.connection)?;
                self.history.prev = None;
            } else if let Some(old) = w_event.old.and_then(|o| o.name) {
                // Case 2: user enters mirror workspace from a normal one
                // -> remember old workspace and redirect away from mirror
                self.history.redirect_from_mirror(&mut self.connection)?;
                self.history.prev = Some(old);
            }
        } else if let Some(_prev) = &self.history.prev {
            // Go to mirror workspace where it was not focused so we should
            // skip the redirection
            let ws_output_current = self.find_output_by_name(&current_name)?;
            let mirror_ws_name = self.mirror_ws.clone();
            let ws_output_mirror = self.find_output_by_name(&mirror_ws_name)?;

            if ws_output_current == ws_output_mirror {
                self.history.skip_next = true;
                Self::move_to_workspace(&self.mirror_ws, &mut self.connection)?;
                Self::move_to_workspace(&current_name, &mut self.connection)?;
                self.history.prev = None;
            }
        }

        Ok(())
    }

    fn handle_output_event(&mut self) -> Fallible<()> {
        let outs = self.connection.get_outputs()?;
        if outs.is_empty() {
            return Ok(());
        }

        let active_outputs: Vec<_> = outs.iter().filter(|o| o.active).collect();
        if active_outputs.len() < 2 {
            println!("[INFO] No secondary screen detected, disabling moving behavior.");
            self.active = false;
            self.mirrored_outputs.clear();
            return Ok(());
        }

        let primary_output = outs
            .iter()
            .find(|o| o.focused)
            .map(|o| o.name.clone())
            .unwrap_or_else(|| outs[0].name.clone());

        for o in &outs {
            // Skip primary output or already mirrored outputs
            let name = &o.name;
            if name == &primary_output || !o.active || self.mirrored_outputs.contains(name) {
                continue;
            }

            // Switch to mirror workspace then launch wl_mirror and go back to
            // previous workspace
            println!("[INFO] Detected new secondary active output: {}", name);
            let current_ws = self.get_focused_workspace()?;
            Self::move_to_workspace(&self.mirror_ws, &mut self.connection)?;

            if let Err(e) = self.launch_wl_mirror(name) {
                eprintln!("[ERROR] Failed to launch wl-mirror for {}: {}", name, e);
                continue;
            }
            self.mirrored_outputs.insert(name.clone());
            self.active = true;
            Self::move_to_workspace(&current_ws, &mut self.connection)?;
        }
        Ok(())
    }

    fn run(mut self) -> Fallible<()> {
        // Startup check
        self.handle_output_event()?;

        // Event loop
        let subs = [EventType::Workspace, EventType::Output];
        for event in Connection::new()?.subscribe(subs)? {
            match event? {
                Event::Workspace(w) if w.change == WorkspaceChange::Focus => {
                    self.handle_workspace_event(*w)?;
                }
                Event::Output(OutputEvent {
                    change: OutputChange::Unspecified,
                    ..
                }) => {
                    self.handle_output_event()?;
                }
                _ => {}
            }
        }

        Ok(())
    }
}

fn main() -> Fallible<()> {
    let args: Vec<String> = env::args().collect();
    let mirror_ws = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| DEFAULT_MIRROR_WS.to_string());
    let mirror_args = if let Some(idx) = args.iter().position(|x| x == "--") {
        args[(idx + 1)..].to_vec()
    } else {
        Vec::new()
    };

    println!("[INFO] Using mirror workspace {}", mirror_ws);
    if !mirror_args.is_empty() {
        println!("[INFO] wl-mirror will be launched with: {:?}", mirror_args);
    }

    MirrorManager::new(mirror_ws, mirror_args)?.run()
}
