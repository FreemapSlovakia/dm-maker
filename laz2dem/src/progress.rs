use std::{
    collections::{HashMap, HashSet},
    time::{Duration, SystemTime},
};

use itertools::iproduct;
use maptile::tile::Tile;

use crate::shared_types::Job;

#[derive(Debug)]
enum State {
    /// no job yet
    Planned,
    /// queued job
    Queued,
    /// job removed from queue, processed
    Processing,
    /// done
    Finished,
}

pub struct Progress {
    supertile_zoom_offset: u8,
    pub jobs: Vec<Job>,
    states: HashMap<Tile, State>,
    last_log: SystemTime,
    done_count: usize,
    buffer: u8,
}

impl Progress {
    pub fn new(jobs: Vec<Job>, supertile_zoom_offset: u8, buffer: u8) -> Self {
        let mut states: HashMap<Tile, State> = jobs
            .iter()
            .flat_map(|job| job.tile().descendants(supertile_zoom_offset))
            .map(|tile| (tile, State::Queued))
            .collect();

        let mut next: HashSet<_> = states.keys().copied().collect();

        loop {
            next = next.iter().filter_map(|tile| tile.parent()).collect();

            if next.is_empty() {
                break;
            }

            states.extend(next.iter().map(|tile| (*tile, State::Planned)));
        }

        Self {
            supertile_zoom_offset,
            jobs,
            states,
            last_log: SystemTime::now(),
            done_count: 0,
            buffer,
        }
    }

    pub fn next(&mut self) -> Option<Job> {
        let job = self.jobs.pop();

        if let Some(ref job) = job {
            let tiles = match job {
                Job::Rasterize(tile_meta) => tile_meta.tile.descendants(self.supertile_zoom_offset),
                Job::Overview(tile) => vec![*tile],
            };

            for tile in tiles {
                *self.states.get_mut(&tile).unwrap() = State::Processing;
            }
        }

        job
    }

    pub fn done(&mut self, tile: Tile) {
        self.done_count += 1;

        *self.states.get_mut(&tile).unwrap() = State::Finished;

        let Some(parent) = tile.parent() else {
            return;
        };

        let sector = tile.sector_in_parent(1);

        let dx = parent.x + sector.0 - 1;

        let dy = parent.y + sector.1 - 1;

        // because we use buffer we need appropriate parent siblings too

        let parents = iproduct!(0..=1, 0..=1).map(|(x, y)| Tile {
            zoom: parent.zoom,
            x: dx + x,
            y: dy + y,
        });

        for parent in parents {
            let can_queue_parent = matches!(self.states.get(&parent), Some(&State::Planned))
                && parent
                    .children_buffered(self.buffer)
                    .all(|tile| matches!(self.states.get(&tile), None | Some(&State::Finished)));

            // println!("AAA {tile} {can_queue_parent}");

            if !can_queue_parent {
                continue;
            }

            self.states.insert(parent, State::Queued);

            self.jobs.push(Job::Overview(parent));
        }

        let t = SystemTime::now();

        if t.duration_since(self.last_log).unwrap() > Duration::from_millis(1000) {
            self.last_log = t;

            self.print_stats();
        }
    }

    pub fn print_stats(&self) {
        println!(
            "{}%",
            (self.done_count * 10_000 / self.states.len()) as f64 / 100.0,
        );
    }
}
