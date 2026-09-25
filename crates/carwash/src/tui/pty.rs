//! Tasks run in pseudo-terminals, so tools keep their colors, progress bars and layout.

use super::app::Msg;
use carwash_core::tasks::Task;
use crossbeam_channel::Sender;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::Read;

pub struct PtyJob {
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

fn size((rows, cols): (u16, u16)) -> PtySize {
    PtySize {
        rows: rows.max(2),
        cols: cols.max(10),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Starts `task` on a new terminal; output and exit arrive as [`Msg::JobOutput`] and
/// [`Msg::JobExited`].
pub fn spawn(
    id: u64,
    task: &Task,
    dimensions: (u16, u16),
    tx: Sender<Msg>,
) -> Result<PtyJob, String> {
    let pair = native_pty_system()
        .openpty(size(dimensions))
        .map_err(|e| format!("cannot open a terminal: {e}"))?;
    let mut command = CommandBuilder::new(&task.program);
    command.args(&task.args);
    command.cwd(&task.cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let mut child = pair.slave.spawn_command(command).map_err(|e| {
        let message = e.to_string();
        if message.contains("No such file") || message.contains("not found") {
            format!("`{}` not found", task.program)
        } else {
            message
        }
    })?;
    // The child holds its own handle to the terminal; ours would keep reads from ending.
    drop(pair.slave);
    let killer = child.clone_killer();
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("cannot read the terminal: {e}"))?;

    let output = tx.clone();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if output
                        .send(Msg::JobOutput(id, buffer[..n].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
    std::thread::spawn(move || {
        let result = child
            .wait()
            .map(|status| i32::try_from(status.exit_code()).unwrap_or(-1))
            .map_err(|e| e.to_string());
        let _ = tx.send(Msg::JobExited(id, result));
    });
    Ok(PtyJob {
        master: pair.master,
        killer,
    })
}

impl PtyJob {
    pub fn resize(&self, dimensions: (u16, u16)) {
        let _ = self.master.resize(size(dimensions));
    }

    /// Stops the task. Dropping the terminal afterwards hangs up its whole process group.
    pub fn kill(mut self) {
        let _ = self.killer.kill();
    }
}
