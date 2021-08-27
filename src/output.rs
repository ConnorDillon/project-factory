use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{ChildStderr, ChildStdout};

use log::error;
use serde_json::{Map, Value};

pub static BUFSIZE: usize = 1024 * 1024;

static NEWLINE: u8 = b"\n"[0];

pub struct Output {
    pub item_path: PathBuf,
    pub item_type: String,
    pub plugin_name: String,
    pub data: OutputData,
}

impl Output {
    pub fn new<P: Into<PathBuf>, S: Into<String>>(
        item_path: P,
        item_type: S,
        plugin_name: S,
        data: OutputData,
    ) -> Output {
        Output {
            item_path: item_path.into(),
            item_type: item_type.into(),
            plugin_name: plugin_name.into(),
            data,
        }
    }

    pub fn handle<T: Write>(self, exit: &mut T) -> io::Result<()> {
        match self.data {
            OutputData::File(path) => copy_output(
                self.plugin_name,
                self.item_path,
                self.item_type,
                &mut BufReader::with_capacity(BUFSIZE, File::open(path)?),
                exit,
            ),
            OutputData::Stdout(out) => copy_output(
                self.plugin_name,
                self.item_path,
                self.item_type,
                &mut BufReader::with_capacity(BUFSIZE, out),
                exit,
            ),
            OutputData::Stderr(err) => log_output(
                &mut BufReader::with_capacity(BUFSIZE, err),
                &self.plugin_name,
            ),
        }
    }
}

pub enum OutputData {
    File(PathBuf),
    Stdout(ChildStdout),
    Stderr(ChildStderr),
}

fn log_output<T: BufRead>(output: &mut T, plugin_name: &str) -> io::Result<()> {
    let mut buf = String::new();
    while output.read_line(&mut buf)? > 0 {
        error!("PLUGIN {}: {}", plugin_name, buf.trim());
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
        in_buf.pop();
        let data = match serde_json::from_str(&in_buf) {
            Ok(x) => Value::Object(x),
            Err(_) => Value::String(in_buf.clone()),
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