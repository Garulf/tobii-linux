# Control channel

The Tobii Experience app (tray, settings, gaze bubble) and `tobii-linux-setup`
talk to a running `tobii-linux serve` over a small control channel on the
service's relay TCP port (default `127.0.0.1:7760`). This is tobii-linux's own
protocol. Tobii's runtime has nothing like it: on Windows the Experience app
and the setup tools reach the service through the platform runtime's PRP
property and calibration commands, whose write layouts have not been recovered
([experience-app.md](experience-app.md)). Games never use this channel; they
still see Tobii's real property 4 (device paused) and its notifications over
PRP ([prp-bridge.md](prp-bridge.md)).

The messages and a blocking client are `crates/tobii-control`; the service side
is `crates/tobii-linux/src/service/control.rs`.

Evidence labels:

- **unit**: `crates/tobii-control/src/protocol.rs` tests, no network.
- **service**: `crates/tobii-linux/tests/control.rs`, the blocking client
  against the service over loopback TCP, fed by scripted trackers (no USB).

## Frames

Control connections use the relay framing of the bridge (`u32 type, u32
length, payload`, little-endian, payload at most 1 MiB,
[prp-bridge.md](prp-bridge.md#relay-protocol)). Types 8 to 12 follow the
bridge's 1 to 7 on the same port.

| Type | Name | Direction | Payload |
| --- | --- | --- | --- |
| 8 | `CONTROL_HELLO` | client to service, first frame | JSON `{"version":1,"client":"experience"}`; `client` is `experience` or `setup` and only appears in the log |
| 9 | `CONTROL_REQUEST` | client to service | JSON request with a client-chosen `id` |
| 10 | `CONTROL_REPLY` | service to client | JSON reply echoing the request's `id` |
| 11 | `CONTROL_EVENT` | service to client | JSON event |
| 12 | `CONTROL_GAZE` | service to client, while subscribed | 28 bytes: i64 timestamp (µs), f64 x, f64 y, u32 valid |

A HELLO with another version, a frame of another type after the HELLO, or a
request that is not JSON with an `id` makes the service close the connection
(logged as a warning).

Only loopback peers (`127.0.0.0/8`, `::1`, and IPv4-mapped loopback) may open a
control connection. With `serve --host 0.0.0.0` the relay port is reachable
from the network for the game bridge, and any host could otherwise release
the tracker, turn eye tracking off, switch profiles or read gaze. A
`CONTROL_HELLO` from another address is logged as a warning and the
connection closed before any reply; bridge and pipe connections from the same
address are served as before (service: `control_refuses_non_loopback_peers`;
unit: `only_loopback_peers_control` in `control.rs`).

A request with an `id` that does not decode is answered
with an error reply.

## Requests

Every request is answered by exactly one reply. Requests on one connection are
handled in the order sent, one at a time, so replies come back in request
order. A reply carries either the service status after the request or an error:

```json
{"id":1,"request":"status"}
{"id":1,"status":{"attached":true,"enabled":true,"released":false,"display_area":true,"profile":"Default","calibration_matches_profile":false}}

{"id":2,"request":{"switch_profile":{"name":"Nobody"}}}
{"id":2,"error":"No profile named Nobody"}
```

| Request | JSON | Effect | Reaches the device |
| --- | --- | --- | --- |
| Status | `"status"` | none | no |
| SetEnabled | `{"set_enabled":false}` | stores the on/off state, then pauses or resumes the tracker for every client, as the tray's toggle does | yes: the running session sends 3100 (pause 1, resume 0); a session that starts later pauses first |
| SwitchProfile | `{"switch_profile":{"name":"Other"}}` | puts a listed profile on the tracker and makes it current, as a click in Tobii's profile list; the current name does nothing; unknown names and `Guest` (never listed) are refused | yes: 3160 with the profile's tracked eyes, then 1110 with its calibration for the recorded monitor unless the device already reports that calibration's id |
| SetTrackedEyes | `{"set_tracked_eyes":"both"}` (`both`, `left`, `right`) | stores the current profile's tracked eyes and applies the profile again, as Tobii's radios do; an unchanged value is applied too, since the device may hold another eye (Tobii only applies a change: the Linux service cannot read the device's eye, and setup or `profiles switch` may have changed it) | yes: 3160, then 1110, even when the ids match |
| Release | `"release"` | lends the tracker to this client (see below) | closes USB |
| Reacquire | `"reacquire"` | ends a release held by this client | the next session opens USB |
| SubscribeGaze | `{"subscribe_gaze":true}` | starts or stops `CONTROL_GAZE` frames on this connection | no |

`SetEnabled` fails with an error reply when the state cannot be stored; nothing
changes then. `SwitchProfile` and `SetTrackedEyes` fail with an error reply
while no tracker is attached or it is released, and when the apply fails; a
failed apply leaves `calibration_matches_profile` false.

When a tracker session starts (plug-in, service start, reacquire after a
release), the service puts the current profile on it, as the EyeX engine does
at start (`TrySetCalibrationAndProfileSettings`): 3160 with the profile's
tracked eyes every time, then 1110 only when the device reports another
calibration id. So a profile that `tobii-linux-setup` (`-N`, `-G`) or the
Python `profiles switch` made current reaches the device with its own eyes
even when its calibration is already there. Nothing is sent without a current
profile, and a failure is logged without stopping the service (service).
Profile storage is in docs/gaze-calibration.md (Calibration
profiles).

The experience app (`crates/tobii-experience/src/link.rs`) waits 2 s for a
reply, except for `SwitchProfile` and `SetTrackedEyes`, which reach the
device: one apply can take 24 s on the service (1490, 3160 and 1490 with the
3 s device-call timeout, 1110 with `TRANSFER_TIMEOUT_S` of 15 s), and a request
may first wait for the apply of an attach, so the app waits 50 s for those. A
reply that does not come in time drops the connection, which the tray shows as
an error until the reconnect (unit, and `a_slow_profile_switch_keeps_the_connection`
in `crates/tobii-experience/tests/link.rs`).

Tracked eyes use Tobii's registry encoding `both 0, left 1, right 2`
(`TrackedEyes::registry_value`) and the device's eye selector `both 3, left 1,
right 2` (`TrackedEyes::device_eye`).

## Status and events

| Field | Meaning |
| --- | --- |
| `attached` | a tracker session runs; false while released |
| `enabled` | eye tracking is on |
| `released` | a control client holds the tracker |
| `display_area` | the service has a display area, from `--display` or read from the tracker |
| `profile` | the current profile name from the profile store, or `null` |
| `calibration_matches_profile` | the last apply of the current profile on the attached tracker succeeded; false while detached or released, before the apply finishes and after a failed one |

Right after accepting the HELLO the service sends one status event, so a client
learns the state without asking:

```json
{"status":{"attached":false,"enabled":true,"released":false,"display_area":false,"profile":null,"calibration_matches_profile":false}}
```

After that it sends a status event to every control connection whenever the
status differs from the last one that connection was sent: on tracker attach
and detach, on/off, release and reacquire, a display area read at attach, and
after each profile apply. The profile is read when an event is due; changing
the profile file alone sends nothing. `ControlClient` skips events it cannot
decode, so a service may add event kinds; a field added to the status needs a
default for older services: `Status` decodes missing fields as their defaults
(`false`, no profile), so a newer app reads an older service's status (unit).
When a request changes the status, its event comes before its reply.
Mapping the status to Tobii's device states (Configuring while released, Device
not connected while detached) is the app's job.

Control connections are not part of the game clients: they stay open when the
tracker is lost, while it is released and while eye tracking is off (service).

## Gaze

`CONTROL_GAZE` carries the gaze point record games get as PRP stream 3: x and y
normalized to the display (0,0 top left, 1,1 bottom right), valid when the
device reported a combined gaze point. The timestamp is the service's host
clock in microseconds. Frames are sent while the connection is subscribed,
whether or not `serve` serves gaze to games (`--no-gaze`); a slow client misses
frames instead of queueing them (service).

## Release lifecycle

`tobii-linux-setup` opens the tracker's USB device itself, so it asks the
service to let go first:

1. Setup connects (`client` `setup`) and sends `release`.
2. The service holds its release gate and sends a status event with
   `released: true`, `attached: false`. The running tracker session sees the
   gate, unsubscribes its streams and closes USB; games see the tracker as
   lost, as on an unplug.
3. The service replies once the session has closed USB, so setup can open the
   device as soon as the reply arrives (service).
4. While the gate is held the service opens no session; its retry loop waits
   on the gate (service).
5. Setup sends `reacquire`, or its connection closes for any reason, including
   a crash. The gate opens, the service reports `released: false`, and the
   next session starts within the retry delay (service).

Only the client holding the release can reacquire; a second `release` or a
`reacquire` from another client gets an error reply (service).

`tobii-linux-setup` connects with a 1 s timeout (a refused connection fails at
once, and then it opens USB directly as without a service), waits up to 15 s for
the release reply, and keeps the connection open until its flow ends, after its
own USB session closed. An error reply, such as another client holding the
release, ends the setup with that error instead of opening USB.
`--no-service` skips the request and `--simulate` never connects.

## What remains

- Not run against a real tracker or the GTK apps yet.
