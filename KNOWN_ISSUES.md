# Known Issues

No open issues.

## Resolved

### Trustless networked deal: intermittent liveness stall

**Was:** ~15–30% of trustless (mental-poker) hands over real libp2p hung at showdown — one peer
never received a reveal token it needed, so it never settled and the host's hand-completion
ack-barrier waited forever. The deal rounds (keygen, sequential shuffle, threshold reveals) require
reliable, every-peer delivery but rode on gossipsub (lossy, best-effort pub/sub) in a star topology
where a guest reaches another guest only via the host's mesh-forward.

**Fix:** the deal payloads (`KeyAnnounce` / `ShuffleAnnounce` / `RevealAnnounce`) now ride a
dedicated libp2p **request-response** protocol (`net`: `DEAL_PROTOCOL`, `NodeHandle::send_deal` /
`NodeEvent::DealReceived`) instead of gossipsub. The host fans every verified payload out to each
other guest, retrying until delivered; the original author is carried in the envelope so anti-cheat
attribution is unchanged (the embedded proof still binds each payload to its author's seat key).
Reliable transport alone is not sufficient — a payload can arrive before the receiver's deal has
reached the phase that can apply it — so each receiver **defers** an out-of-phase payload and
replays it as its deal advances (`driver`: `apply_deal_payload` / `replay_deferred`). Betting
`Act`, `StartHand`/`StartMentalHand`, `HandComplete`, and lobby `JoinTable` stay on gossipsub.

Verified by `cargo test -p poker-protocol --test networked_hand` (the 2-peer and 3-peer trustless
tests, formerly `#[ignore]`d, now run by default) — 40 consecutive loop runs (80 hands) with zero
stalls.
