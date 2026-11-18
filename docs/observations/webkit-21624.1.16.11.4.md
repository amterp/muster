# WebKit canvas isolation on macOS 26.4.1

Measured on 2026-11-18, on macOS 26.4.1 (25E253) and WebKit 21624.1.16.11.4. The spike used
an accessory app with a non-persistent data store, a content rule list installed before
loading, and a navigation delegate that allowed only the host's initial document load.
Listeners on loopback counted HTTP requests, TCP connections, and UDP datagrams separately.
Run names below identify the evidence in the spike report, `report-4.md`. The repository keeps
these observations. The raw transcripts were not checked in because the gate has no spike driver.

## 1. Page scripts and network traffic

The spike tested the proposed rules with scripts on and off, using controls that reached
the listener. Protected runs observed each vector for 3 to 12 seconds. These are measurements
of the named fixtures on one OS release.

| Observation | Evidence in the spike report |
|---|---|
| The resource rules blocked the tested remote images, stylesheets, fonts, preloads, and media. The controls reached those endpoints. | Network results, `run-1/resources-strict`, `run-1/resources-off`, and `run-2/resources-control-off`. |
| The same rules blocked the tested fetch, XHR, EventSource, beacon, and WebSocket requests with scripts on. | Network results, `run-1/api-strict` and `run-4/ws-strict`, compared with their controls. |
| With scripts on, WebRTC sent a UDP datagram to the STUN listener. The scripts-off run recorded zero datagrams. | Network results, `run-1/rtc-strict` and `run-1/rtc-off`. |
| Clicking a plain HTTP link with scripts off opened a TCP connection even though the delegate canceled navigation. The listener saw no HTTP request. A document-start click handler that prevented the default action produced zero accepts in the fixture. | Link activation, `run-3/link-off` and `run-3/link-cancel-click-off`. |

The WebRTC result informed the decision to keep page scripts off. The link result informed
the shell's click interception. The shell also cancels navigation and opens allowed destinations
through the system when the person clicks them. The spike used loopback listeners and host-driven
clicks. It measured neither system DNS traffic nor interactive foreground activation.
See the [canvas web view decision](../../.fencil/decisions/canvas-web-view.toml) and the
[macOS browser implementation](../../Sources/MusterMac/WKCanvasBrowser.swift).

## 2. Rules must be installed before loading

Malformed rules returned `WKErrorDomain 6` and the probe recorded `NO_LOAD`. A deliberate
late install let the first image request reach the listener. The same page produced zero
accepts when the probe waited for the compiled rules to be installed before starting the
load. Evidence comes from the spike report's rule-list readiness section, `smoke-gui.log`
and `run-2/resources-pending`.

## 3. Keyboard routing

The test window saw Cmd-W before WebKit. It consumed the chord while Down and Space reached
the page and scrolled it. The host identified a mouse-down over the view in the inactive
window. A direct Copy action reached the focused test view. The foreground Cmd-C chord and
pasteboard contents remained unverified. Evidence comes from the keyboard routing section,
`run-4/keys.log`, `run-4/keys-off.log`, and `keys-appkit-key-window.log`.

## 4. Rendering time and memory

Three fresh app processes each created five web views with page scripts off. The views shared
one non-persistent data store and used compiled rules. The stopwatch stopped when the new
page color appeared in a WebKit snapshot. The measurement excludes daemon and SSH reads and
physical display presentation. The machine was running other jobs under Quartermaster.
Evidence comes from the cost section, `bench-steady-1.log` through `bench-steady-3.log` and
`bench-summary.txt`.

| Operation | Samples | Median | Range |
|---|---:|---:|---:|
| First web view in a fresh process | 3 | 249 ms | 239–630 ms |
| Additional web view in the same process | 12 | 55 ms | 53–60 ms |
| Replace a 100,000-byte HTML page | 36 | 53 ms | 51–59 ms |

Each view had a separate WebContent process. Median WebContent footprint was 11.00 MiB for
one view and 53.63 MiB summed across five views. The footprint totals exclude the app, GPU,
and networking processes. Evidence comes from the same three raw benchmark runs and their
computed summary.
