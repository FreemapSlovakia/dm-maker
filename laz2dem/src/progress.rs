use crate::shared_types::Job;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, SystemTime},
};
use tilemath::tile::Tile;

enum State {
    Planned,    // no job yet
    Waiting,    // queued job
    Processing, // job removed from queue, processed
    Finished,   // done
}

pub struct Progress {
    supertile_zoom_offset: u8,
    pub jobs: Vec<Job>,
    states: HashMap<Tile, State>,
    last_log: SystemTime,
    done_count: usize,
}

impl Progress {
    pub fn new(jobs: Vec<Job>, supertile_zoom_offset: u8) -> Self {
        let mut states: HashMap<Tile, State> = jobs
            .iter()
            .flat_map(|job| job.tile().descendants(supertile_zoom_offset))
            .map(|tile| (tile, State::Waiting))
            .collect();

        let mut next: HashSet<_> = states.keys().copied().collect();

        loop {
            states.extend(next.iter().map(|tile| (*tile, State::Planned)));

            next = next.iter().filter_map(|tile| tile.parent()).collect();

            if next.is_empty() {
                break;
            }
        }

        Self {
            supertile_zoom_offset,
            jobs,
            states,
            last_log: SystemTime::now(),
            done_count: 0,
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
        *self.states.get_mut(&tile).unwrap() = State::Finished;

        let Some(parent) = tile.parent() else {
            return;
        };

        let parent_state = self.states.get(&parent);

        if !matches!(parent_state, Some(&State::Planned))
            || parent.children().iter().any(|tile| {
                matches!(
                    self.states.get(tile),
                    Some(&State::Processing | &State::Waiting | &State::Planned)
                )
            })
        {
            return;
        }

        self.states.insert(parent, State::Waiting);

        self.jobs.push(Job::Overview(parent));

        let t = SystemTime::now();

        self.done_count += 1;

        if t.duration_since(self.last_log).unwrap() > Duration::from_millis(1000) {
            self.last_log = t;

            println!(
                "{}% {} {}",
                (self.done_count * 10_000 / self.states.len()) as f64 / 100.0,
                self.jobs.len(),
                self.done_count,
            );
        }
    }
}
