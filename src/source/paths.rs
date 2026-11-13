//! `scry paths [--target TARGET] OLD NEW` (spec 4.2): two toplevels on one machine, resolved
//! there, nothing built.

use std::time::Duration;

use crate::cli::PathsArgs;
use crate::fetch::{Job, Toplevel, Wanted};
use crate::report::Source;
use crate::transport::machine::{Machine, Remote};

/// The one configuration of the run, named after its machine: `TARGET`, or `localhost`.
pub fn job(args: &PathsArgs) -> Job {
    let machine = match &args.target {
        Some(target) => Machine::Remote(Remote {
            target: target.clone(),
            port: None,
            ssh_options: Vec::new(),
        }),
        None => Machine::Local,
    };
    let side = |path: &str| Wanted {
        machine: machine.clone(),
        toplevel: Toplevel::Path {
            path: path.to_owned(),
            build: None,
        },
    };
    Job {
        name: args
            .target
            .as_ref()
            .map_or_else(|| "localhost".to_owned(), ToString::to_string),
        source: Source::Paths,
        old: side(&args.old),
        new: side(&args.new),
        max_file_size: args.common.max_file_size,
        timeout: Duration::from_secs(args.common.timeout),
    }
}
