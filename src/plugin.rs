use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub type Config = HashMap<FileType, Plugin>;

pub type FileType = String;

#[derive(Debug, Deserialize)]
pub struct Plugin {
    pub name: String,
    pub path: PathBuf,
    pub args: Option<Vec<String>>,
    pub input: Option<InputType>,
    pub output: Option<OutputType>,
    pub unpacker: Option<bool>,
}

#[derive(Copy, Clone, Debug, Deserialize, PartialEq)]
#[allow(non_camel_case_types)]
pub enum InputType {
    file,
    stdin,
}

#[derive(Copy, Clone, Debug, Deserialize, PartialEq)]
#[allow(non_camel_case_types)]
pub enum OutputType {
    file,
    dir,
    stdout,
}

pub struct PreppedProcess {
    pub command: Command,
    pub input_file_name: Option<String>,
    pub output_file_name: Option<String>,
    pub input_type: InputType,
    pub output_type: OutputType,
    pub plugin_name: String,
    pub unpacker: bool,
}

pub fn prep_process(plugin: &Plugin) -> PreppedProcess {
    let mut cmd = Command::new(&plugin.path);
    let mut args = plugin.args.clone().unwrap_or(Vec::new());
    let input_type = plugin.input.unwrap_or(InputType::file);
    let output_type = plugin.output.unwrap_or(OutputType::file);
    let input_file_name = match input_type {
        InputType::stdin => {
            cmd.stdin(Stdio::piped());
            None
        }
        InputType::file => {
            cmd.stdin(Stdio::null());
            let path = gen_io_path().unwrap();
            cmd.env("INPUT", &path);
            replace_arg(&mut args, "$INPUT", &path);
            Some(path)
        }
    };
    let output_file_name = match output_type {
        OutputType::stdout => None,
        OutputType::dir => Some(gen_io_path().unwrap()),
        OutputType::file => Some(gen_io_path().unwrap()),
    };
    if let Some(path) = &output_file_name {
        cmd.env("OUTPUT", path);
        replace_arg(&mut args, "$OUTPUT", path);
    }
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    PreppedProcess {
        command: cmd,
        input_file_name,
        output_file_name,
        input_type,
        output_type,
        plugin_name: plugin.name.clone(),
        unpacker: plugin.unpacker.unwrap_or(false),
    }
}

fn replace_arg(args: &mut Vec<String>, var: &str, rep: &str) {
    let idxs = args
        .iter()
        .enumerate()
        .filter(|x| x.1 == var)
        .map(|x| x.0)
        .collect::<Vec<usize>>();
    for idx in idxs {
        args.remove(idx);
        args.insert(idx, rep.to_string());
    }
}

fn gen_io_path() -> io::Result<String> {
    let mut path = env::current_dir()?;
    let r: u64 = rand::random();
    let name = format!("{:016x}", r);
    path.push(name);
    Ok(path.to_str().unwrap().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prep_process() {
        let plugin = Plugin {
            name: "foo".into(),
            path: "bar".into(),
            args: Some(vec!["--baz".into(), "$INPUT".into()]),
            input: None,
            output: Some(OutputType::stdout),
            unpacker: None,
        };
        let proc = prep_process(&plugin);
        assert_eq!(
            proc.input_file_name,
            proc.command
                .get_args()
                .nth(1)
                .and_then(|x| x.to_str())
                .map(String::from)
        );
        assert!(proc.output_file_name.is_none());
    }
}
