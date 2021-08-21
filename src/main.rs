#![feature(command_access)]

use getopts::Options;
use log::debug;
use process::Input;
use serde_yaml::from_reader;
use std::fs::File;
use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::{env, fs};
use yara::{Compiler, Rules};

use crate::plugin::Config;
use crate::process::Global;

mod plugin;
mod process;

fn main() {
    env_logger::init();
    let opts = set_opts();
    let args: Vec<String> = env::args().collect();
    let params = read_params(&opts, &args);
    if params.help {
        print!("{}", opts.usage("Usage: factory [options]"));
    } else if let (Some(cpath), Some(ypath)) = (params.config, params.yara) {
        let cfile = File::open(cpath).unwrap();
        let conf: Config = from_reader(cfile).unwrap();
        debug!("Config: {:?}", conf);
        let mut compiler = Compiler::new().unwrap();
        compiler.add_rules_file(ypath).unwrap();
        let rules = compiler.compile_rules().unwrap();
        execute(params.input, conf, rules, Output(io::stdout())).unwrap();
    } else {
        print!("{}", opts.usage("Usage: factory [options]"));
    }
}

fn execute<T>(input: Option<PathBuf>, config: Config, rules: Rules, output: T) -> io::Result<()>
where
    T: Write + Clone + Send + 'static,
{
    let cpus = num_cpus::get();
    let global = Global::new(config, rules, cpus, cpus * 2);
    let input_path = match input {
        Some(p) => Some(fs::canonicalize(p)?),
        None => None,
    };
    let working_dir = plugin::gen_path()?;
    fs::create_dir(&working_dir).unwrap();
    env::set_current_dir(&working_dir)?;
    if let Some(path) = input_path {
        if path.is_dir() {
            process::process_dir(global.clone(), path, false, output)?;
        } else {
            process::process_file(global.clone(), path, false, output)?;
        }
    } else {
        process::process_input(global.clone(), Input::new("", io::stdin()), None, output)?;
    }
    global.join();
    env::set_current_dir(&working_dir.parent().unwrap())?;
    fs::remove_dir_all(working_dir).unwrap();
    Ok(())
}

fn set_opts() -> Options {
    let mut opts = Options::new();
    opts.optflag("h", "help", "Show this help information.");
    opts.optopt(
        "y",
        "yara",
        "Path to the yara rules file (required)",
        "PATH",
    );
    opts.optopt("c", "config", "Path to the config file (required)", "PATH");
    opts.optopt(
        "i",
        "input",
        "Path to the input file (will read from stdin if not specified)",
        "PATH",
    );
    opts
}

fn read_params(opts: &Options, args: &Vec<String>) -> Params {
    let matches = opts.parse(&args[1..]).unwrap();
    Params {
        help: matches.opt_present("help"),
        config: matches.opt_get("config").unwrap(),
        yara: matches.opt_get("yara").unwrap(),
        input: matches.opt_get("input").unwrap(),
    }
}

struct Params {
    help: bool,
    config: Option<PathBuf>,
    yara: Option<PathBuf>,
    input: Option<PathBuf>,
}

struct Output(Stdout);

impl Clone for Output {
    fn clone(&self) -> Output {
        Output(io::stdout())
    }
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.write_all(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
