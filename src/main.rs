#![feature(command_access)]

use getopts::Options;
use log::debug;
use serde_yaml::from_reader;
use std::env;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use tar::Archive;
use yara::Compiler;

use crate::plugin::Config;

mod plugin;
mod process;

fn main() {
    env_logger::init();
    let opts = set_opts();
    let args: Vec<String> = env::args().collect();
    let params = read_params(&opts, &args);
    if params.help {
        print!("{}", opts.usage("Usage: factory [options]"));
    } else if let (Some(cpath), Some(ypath), Some(ipath)) =
        (params.config, params.yara, params.input)
    {
        let cfile = File::open(cpath).unwrap();
        let conf: Config = from_reader(cfile).unwrap();
        debug!("Config: {:?}", conf);
        let mut compiler = Compiler::new().unwrap();
        compiler.add_rules_file(ypath).unwrap();
        let rules = compiler.compile_rules().unwrap();
        let ifile = File::open(ipath).unwrap();
        let mut archive = Archive::new(ifile);
        process::process_files(
            &conf,
            archive.entries().unwrap(),
            |_| Ok(io::stdout()),
            rules,
        )
        .unwrap();
    }
}

fn set_opts() -> Options {
    let mut opts = Options::new();
    opts.optflag("h", "help", "Show this help information.");
    opts.optopt("y", "yara", "Path to the yara rules file", "PATH");
    opts.optopt("c", "config", "Path to the config file", "PATH");
    opts.optopt("i", "input", "Path to the input file", "PATH");
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
