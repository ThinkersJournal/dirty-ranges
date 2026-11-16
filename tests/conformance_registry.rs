// SPDX-License-Identifier: MIT OR Apache-2.0
//! The README's "at most 64 regions per process". The registry is one process-wide table, so this lives in its
//! own test binary (its own process) and is a single test: filling it inside the lib tests would starve every
//! other test that arms a region.
#![cfg(any(unix, windows))]

use dirty_ranges::{Error, Region};

const LIMIT: usize = 64;

fn arm(map: &mut memmap2::MmapMut) -> Result<Region, Error> {
    // SAFETY: a live anonymous mapping, page-aligned and whole pages, that outlives the returned region
    // (the caller drops every region before its mapping); no alias; nothing reads into it from the OS.
    unsafe { Region::arm(map.as_mut_ptr(), map.len(), 2, None) }
}

#[test]
fn the_sixty_fifth_region_is_refused_and_a_freed_slot_is_reusable() {
    // 64 KiB is whole pages for every page size we support (4, 16 and 64 KiB).
    let size = 1 << 16;
    let mut maps: Vec<memmap2::MmapMut> = (0..=LIMIT)
        .map(|_| memmap2::MmapMut::map_anon(size).unwrap())
        .collect();
    let mut last = maps.pop().unwrap();

    let mut regions: Vec<Region> = Vec::new();
    for (i, map) in maps.iter_mut().enumerate() {
        match arm(map) {
            Ok(region) => regions.push(region),
            Err(e) => panic!("region {} of {LIMIT} was refused: {e}", i + 1),
        }
    }
    assert_eq!(regions.len(), LIMIT);

    match arm(&mut last) {
        Err(Error::RegistryFull) => {}
        Err(e) => panic!("the 65th region failed with the wrong error: {e}"),
        Ok(_) => panic!("the 65th region was accepted: the limit is not {LIMIT}"),
    }

    // Dropping one frees its slot: the refused arm leaves nothing behind and the next one fits.
    drop(regions.pop());
    let again = arm(&mut last).expect("a freed slot must be reusable");
    drop(again);
    drop(regions);
}
