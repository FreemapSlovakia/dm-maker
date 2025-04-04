mod options;
mod progress;
mod rasterization;
mod read;
mod schema;
mod shading;
mod shared_types;
mod vector_resampling;

use std::fs::{exists, remove_file};

use clap::Parser;
use options::{ExistingFileAction, Options};
use rasterization::rasterize;
use read::read;
use shared_types::Job;

fn main() {
    let options = Options::parse();

    let r#continue = exists(&options.output).unwrap()
        && match options.existing_file_action {
            Some(ExistingFileAction::Overwrite) => {
                remove_file(&options.output).unwrap();

                false
            }
            Some(ExistingFileAction::Continue) => true,
            None => panic!("Output file already exitsts. Specify --existing-file-action."),
        };

    let tile_metas = read(&options);

    let mut jobs: Vec<_> = tile_metas.into_iter().map(Job::Rasterize).collect();

    jobs.sort_by_cached_key(|job| job.tile().morton_code());

    rasterize(&options, r#continue, jobs);
}
