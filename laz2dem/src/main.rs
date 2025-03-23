mod options;
mod progress;
mod rasterization;
mod read;
mod schema;
mod shading;
mod shared_types;

use clap::Parser;
use options::Options;
use rasterization::rasterize;
use read::read;
use shared_types::Job;

fn main() {
    let options = Options::parse();

    let tile_metas = read(&options);

    let mut jobs: Vec<_> = tile_metas.into_iter().map(Job::Rasterize).collect();

    jobs.sort_by_cached_key(|job| job.tile().morton_code());

    rasterize(&options, jobs);
}
