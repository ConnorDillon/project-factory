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

impl Plugin {
    pub fn prep(&self, file_path: Option<&PathBuf>) -> io::Result<PreppedPlugin> {
        let mut cmd = Command::new(&self.path);
        let mut args = self.args.clone().unwrap_or(Vec::new());
        let input_type = self.input.unwrap_or(InputType::file);
        let output_type = self.output.unwrap_or(OutputType::file);
        let input_path = match input_type {
            InputType::stdin => {
                cmd.stdin(Stdio::piped());
                InputPath::Stdin
            }
            InputType::file => {
                cmd.stdin(Stdio::null());
                let path = file_path.map(|x| x.clone()).unwrap_or(gen_path()?);
                cmd.env("INPUT", &path);
                replace_arg(&mut args, "$INPUT", &path.to_str().unwrap());
                InputPath::File(path)
            }
        };
        let output_path = match output_type {
            OutputType::stdout => OutputPath::Stdout,
            OutputType::dir => {
                let path = gen_path()?;
                cmd.env("OUTPUT", &path);
                replace_arg(&mut args, "$OUTPUT", path.to_str().unwrap());
                cmd.current_dir(&path);
                OutputPath::Dir(path)
            }
            OutputType::file => {
                let path = gen_path()?;
                cmd.env("OUTPUT", &path);
                replace_arg(&mut args, "$OUTPUT", path.to_str().unwrap());
                OutputPath::File(path)
            }
        };
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        Ok(PreppedPlugin {
            plugin_name: self.name.clone(),
            command: cmd,
            input_path,
            output_path,
            unpacker: self.unpacker.unwrap_or(false),
        })
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

pub fn gen_path() -> io::Result<PathBuf> {
    let mut path = env::current_dir()?;
    let r: u64 = rand::random();
    let name = format!("{:016x}", r);
    path.push(name);
    Ok(path)
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

#[derive(Debug)]
pub struct PreppedPlugin {
    pub plugin_name: String,
    pub command: Command,
    pub input_path: InputPath,
    pub output_path: OutputPath,
    pub unpacker: bool,
}

#[derive(Debug, PartialEq)]
pub enum InputPath {
    File(PathBuf),
    Stdin,
}

impl InputPath {
    pub fn file(&self) -> Option<&PathBuf> {
        match self {
            InputPath::File(path) => Some(path),
            InputPath::Stdin => None,
        }
    }

    pub fn stdin(&self) -> bool {
        self == &InputPath::Stdin
    }
}

#[derive(Debug, PartialEq)]
pub enum OutputPath {
    Dir(PathBuf),
    File(PathBuf),
    Stdout,
}

impl OutputPath {
    pub fn dir(&self) -> Option<&PathBuf> {
        match self {
            OutputPath::Dir(path) => Some(path),
            _ => None,
        }
    }

    pub fn stdout(&self) -> bool {
        self == &OutputPath::Stdout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prep() {
        let plugin = Plugin {
            name: "foo".into(),
            path: "bar".into(),
            args: Some(vec!["--baz".into(), "$INPUT".into()]),
            input: None,
            output: Some(OutputType::stdout),
            unpacker: None,
        };
        let prepped = plugin.prep(None).unwrap();
        assert_eq!(
            Some(&prepped.input_path),
            prepped.command
                .get_args()
                .nth(1)
                .and_then(|x| x.to_str())
                .map(|x| InputPath::File(PathBuf::from(x)))
                .as_ref()
        );
        assert!(prepped.output_path.stdout());
        let prepped = plugin.prep(Some(&"/foo/bar".into())).unwrap();
        assert_eq!(
            Some("/foo/bar"),
            prepped.command.get_args().nth(1).and_then(|x| x.to_str())
        );
    }
}
