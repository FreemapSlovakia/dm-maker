use std::collections::{HashMap, HashSet};

use maptile::tile::Tile;

use crate::shared_types::Job;

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
                println!("NEXT {tile}");

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

        if !matches!(parent_state, Some(&State::Planned)) {
            return;
        }

        if parent.children().iter().any(|tile| {
            matches!(
                self.states.get(tile),
                Some(&State::Processing | &State::Waiting | &State::Planned)
            )
        }) {
            println!("HALT {parent}");

            return;
        }

        println!("PASS {parent}");

        self.states.insert(parent, State::Waiting);

        self.jobs.push(Job::Overview(parent));
    }
}
