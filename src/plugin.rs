use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, Read};
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

#[derive(Debug)]
pub struct PreppedProcess {
    pub command: Command,
    temp_input_path: Option<PathBuf>,
    pub output_path: Option<PathBuf>,
    pub input_type: InputType,
    pub output_type: OutputType,
    pub plugin_name: String,
    pub unpacker: bool,
    input_file: Option<InputFile>,
}

impl PreppedProcess {
    #[allow(dead_code)]
    pub fn input_path(&self) -> Option<&PathBuf> {
        self.temp_input_path
            .as_ref()
            .or(self.input_file.as_ref().map(|x| &x.path))
    }

    pub fn prepare_input<T: Read>(&self, input: &mut T) -> io::Result<()> {
        if self.output_type == OutputType::dir {
            fs::create_dir(self.output_path.as_ref().unwrap())?;
        }
        if let Some(path) = &self.temp_input_path {
            io::copy(input, &mut File::create(path)?)?;
        }
        Ok(())
    }

    pub fn cleanup_input(&self) -> io::Result<()> {
        if let Some(path) = &self.temp_input_path {
            fs::remove_file(path)?;
        } else if let Some(infile) = &self.input_file {
            if infile.temp {
                fs::remove_file(&infile.path)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct InputFile {
    pub path: PathBuf,
    pub temp: bool,
}

impl InputFile {
    pub fn new<T: Into<PathBuf>>(path: T, temp: bool) -> InputFile {
        InputFile {
            path: path.into(),
            temp,
        }
    }

    pub fn cleanup(&self) -> io::Result<()> {
	if self.temp {
	    fs::remove_file(&self.path)?;
	}
	Ok(())
    }
}

pub fn prep_process(plugin: &Plugin, input_file: Option<InputFile>) -> PreppedProcess {
    let mut cmd = Command::new(&plugin.path);
    let mut args = plugin.args.clone().unwrap_or(Vec::new());
    let input_type = plugin.input.unwrap_or(InputType::file);
    let output_type = plugin.output.unwrap_or(OutputType::file);
    let temp_input_path = match input_type {
        InputType::stdin => {
            cmd.stdin(Stdio::piped());
            None
        }
        InputType::file => {
            cmd.stdin(Stdio::null());
            let (input_path, temp_path) = if let Some(infile) = &input_file {
                (Some(&infile.path), None)
            } else {
                (None, Some(gen_path().unwrap()))
            };
            let path = input_path.or(temp_path.as_ref()).unwrap();
            cmd.env("INPUT", &path);
            replace_arg(&mut args, "$INPUT", &path.to_str().unwrap());
            temp_path
        }
    };
    let output_path = match output_type {
        OutputType::stdout => None,
        OutputType::dir => {
            let path = gen_path().unwrap();
            cmd.current_dir(&path);
            Some(path)
        },
        OutputType::file => Some(gen_path().unwrap()),
    };
    if let Some(path) = &output_path {
        cmd.env("OUTPUT", path);
        replace_arg(&mut args, "$OUTPUT", path.to_str().unwrap());
    }
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    PreppedProcess {
        command: cmd,
        temp_input_path,
        output_path,
        input_type,
        output_type,
        plugin_name: plugin.name.clone(),
        unpacker: plugin.unpacker.unwrap_or(false),
        input_file,
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
        let proc = prep_process(&plugin, None);
        assert_eq!(
            proc.input_path(),
            proc.command
                .get_args()
                .nth(1)
                .and_then(|x| x.to_str())
                .map(PathBuf::from)
                .as_ref()
        );
        assert!(proc.output_path.is_none());
        let proc = prep_process(&plugin, Some(InputFile::new("/foo/bar", true)));
        assert_eq!(
            Some("/foo/bar"),
            proc.command.get_args().nth(1).and_then(|x| x.to_str())
        );
    }
}
