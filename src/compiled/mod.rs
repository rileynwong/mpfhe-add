#![allow(dead_code)]
#![allow(clippy::all)]

#[rustfmt::skip]
mod get_cell;
#[rustfmt::skip]
mod lay_egg;
#[rustfmt::skip]
mod move_player;
#[rustfmt::skip]
mod pickup_egg;

pub(super) use get_cell::get_cell;
pub(super) use lay_egg::lay_egg;
pub(super) use move_player::move_player;
pub(super) use pickup_egg::pickup_egg;
