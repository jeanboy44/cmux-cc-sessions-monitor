//! The stance acts start from: a port of pixelfx `core/poses.js`. A pose only says what differs from standing easy
//! ([`STAND`](super::rig::STAND)), in screen terms: px from the feet, y negative up. Give the hands and feet; the
//! skeleton works out the rest (see [`Pose`] for the keys).
//!
//! How the body works: he faces right, three-quarters towards us, so his near shoulder is on the left of the chest on
//! screen. An arm reaches about 15 px from its shoulder (shoulders are near y = -29), a leg 17 px. Arms don't stretch.

use super::rig::Pose;

/// Ready to act: knees bent, both hands up in front.
pub const READY: Pose = Pose::EMPTY.crouch(3.0).fx(11.0).fy(-21.0).bx(9.0).by(-21.0).ffx(7.0).fbx(-7.0);
