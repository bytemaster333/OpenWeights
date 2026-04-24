//! `/admin/*` handlers — console admin surface.
//! ( amendment — ). Surface:
//! | Method + Path | Handler |
//! |------------------------------|---------------------------------|
//! | GET /admin/me | `me::get_me` |
//! | POST /admin/keys | `keys::create_key` |
//! | GET /admin/keys | `keys::list_keys` |
//! | DELETE /admin/keys/{id} | `keys::revoke_key` |
//! | GET /admin/stats | `stats::get_stats` |
//! | GET /admin/xorbs | `xorbs::list_xorbs` (admin) |
//! | GET /admin/stats/map | `map::get_map` |
//! | GET /admin/setup/status | `setup::get_setup_status` (adm) |
//! Every handler takes a `Session` extractor — missing/invalid cookie = 401
//! before the handler body runs. Admin-only routes additionally check
//! `session.user.is_admin` in-body and return 403 on non-admin.

pub mod keys;
pub mod map;
pub mod me;
pub mod setup;
pub mod stats;
pub mod xorbs;
