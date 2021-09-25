use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{ChildStderr, ChildStdout};
use std::thread::{self, ThreadId};

use log::{error, info};
use serde_json::{Map, Value};

pub static BUFSIZE: usize = 1024 * 1024;

static NEWLINE: u8 = b"\n"[0];

#[derive(Copy, Clone, Debug)]
pub struct TaskId(ThreadId, u64);

impl TaskId {
    pub fn new(id: u64) -> TaskId {
        TaskId(thread::current().id(), id)
    }
}

impl Display for TaskId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Task({}.{})", self.0.as_u64(), self.1)
    }
}

#[derive(Debug)]
pub struct Output {
    pub task_id: TaskId,
    pub item_path: PathBuf,
    pub item_type: String,
    pub plugin_name: String,
    pub data: OutputData,
}

impl Output {
    pub fn new<P: Into<PathBuf>, S: Into<String>>(
        task_id: TaskId,
        item_path: P,
        item_type: S,
        plugin_name: S,
        data: OutputData,
    ) -> Output {
        Output {
            task_id,
            item_path: item_path.into(),
            item_type: item_type.into(),
            plugin_name: plugin_name.into(),
            data,
        }
    }

    pub fn handle<T: Write>(self, exit: &mut T) -> io::Result<()> {
        match self.data {
            OutputData::File(path) => match File::open(&path) {
                Ok(file) => copy_output(
                    self.plugin_name,
                    self.item_path,
                    self.item_type,
                    &mut BufReader::with_capacity(BUFSIZE, file),
                    exit,
                ),
                Err(err) => {
                    if !path.exists() {
                        error!(
                            "{}: Expected output file does not exist {:?}",
                            self.task_id, path
                        );
                    } else if path.is_dir() {
                        error!(
                            "{}: Expected output file is a dir (check output type in config) {:?}",
                            self.task_id, path
                        );
                    }
                    Err(err)
                }
            },
            OutputData::Stdout(out) => copy_output(
                self.plugin_name,
                self.item_path,
                self.item_type,
                &mut BufReader::with_capacity(BUFSIZE, out),
                exit,
            ),
            OutputData::LogStdout(out) => log_output(
                &mut BufReader::with_capacity(BUFSIZE, out),
                &self.plugin_name,
            ),
            OutputData::LogStderr(err) => log_output(
                &mut BufReader::with_capacity(BUFSIZE, err),
                &self.plugin_name,
            ),
        }
    }
}

#[derive(Debug)]
pub enum OutputData {
    File(PathBuf),
    Stdout(ChildStdout),
    LogStdout(ChildStdout),
    LogStderr(ChildStderr),
}

fn log_output<T: BufRead>(output: &mut T, plugin_name: &str) -> io::Result<()> {
    let mut buf = String::new();
    while output.read_line(&mut buf)? > 0 {
        info!("PLUGIN {}: {}", plugin_name, buf.trim());
        buf.clear();
    }
    Ok(())
}

fn copy_output<T: BufRead, U: Write>(
    plugin_name: String,
    item_path: PathBuf,
    item_type: String,
    output: &mut T,
    mut exit: U,
) -> io::Result<()> {
    let mut in_buf = String::new();
    let mut out_buf = Vec::new();
    let mut map = Map::new();
    map.insert("plugin".into(), plugin_name.into());
    map.insert("path".into(), item_path.to_str().unwrap().into());
    map.insert("type".into(), item_type.into());
    let mut line = Value::Object(map);
    while output.read_line(&mut in_buf)? > 0 {
	let s = in_buf.trim_end();
        let data = match serde_json::from_str(s) {
            Ok(x) => x,
            Err(_) => Value::String(s.to_string()),
        };
        line.as_object_mut().unwrap().insert("data".into(), data);
        serde_json::to_writer(&mut out_buf, &line)?;
        out_buf.push(NEWLINE);
        exit.write_all(&out_buf)?;
        in_buf.clear();
        out_buf.clear();
    }
    Ok(())
}
