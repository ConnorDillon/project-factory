use log::error;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{ChildStderr, ChildStdout};

pub static BUFSIZE: usize = 1024 * 1024;

static NEWLINE: u8 = b"\n"[0];

static COLON: u8 = b":"[0];

pub struct Output {
    pub item_path: PathBuf,
    pub plugin_name: String,
    pub data: OutputData,
}

impl Output {
    pub fn new<T: Into<PathBuf>, U: Into<String>>(
        item_path: T,
        plugin_name: U,
        data: OutputData,
    ) -> Output {
        Output {
            item_path: item_path.into(),
            plugin_name: plugin_name.into(),
            data,
        }
    }

    pub fn handle<T: Write>(self, exit: &mut T) -> io::Result<()> {
        match self.data {
            OutputData::File(path) => copy_output(
                &self.plugin_name,
                &mut BufReader::with_capacity(BUFSIZE, File::open(path)?),
                exit,
            ),
            OutputData::Stdout(out) => copy_output(
                &self.plugin_name,
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
    plugin_name: &str,
    output: &mut T,
    exit: &mut U,
) -> io::Result<()> {
    let mut buf = Vec::new();
    buf.extend_from_slice(plugin_name.as_bytes());
    buf.push(COLON);
    while output.read_until(NEWLINE, &mut buf)? > 0 {
        buf.pop();
        if buf.ends_with(b"\r") {
            buf.pop();
        }
        buf.push(NEWLINE);
        exit.write_all(&buf)?;
        buf.truncate(plugin_name.len() + 1);
    }
    Ok(())
}
