// Muster's shell/core seam.
//
// The whole boundary between a native per-OS shell and the portable core: three functions
// and a callback. Hand-written rather than generated, because this file is the contract -
// a shell on another platform implements against it, and it should be readable without
// building anything.
//
// Everything crossing it is a protobuf message from proto/muster.proto. The seam carries
// events, never bytes: pane output runs adapter to surface and never enters the core, so
// nothing here is on a per-byte path (mip/0001-portable-core.md).

#ifndef MUSTER_H
#define MUSTER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Called by the core when something changed that the shell did not ask about - an agent
// changed state, a pane became typeable, a notification is due.
//
// `bytes` is an encoded Event, valid only for the duration of the call: copy what you
// need. Called from whichever thread noticed, which is never the shell's UI thread, so an
// implementation that touches a window must hop to it.
typedef void (*MusterEventCallback)(const uint8_t *bytes, size_t len);

// Registers the callback, or clears it with NULL. The function must stay callable for as
// long as the process lives.
void muster_set_event_callback(MusterEventCallback callback);

// Answers one encoded Request with one encoded Response.
//
// Writes the response length to `out_len` and returns a buffer the caller owns until it
// passes both back to muster_free. Returns NULL only if the core could not answer at all,
// which means a bug below this line; a request the core understood but refused comes back
// as a Response saying why.
const uint8_t *muster_dispatch(const uint8_t *request, size_t len, size_t *out_len);

// Releases a buffer from muster_dispatch. `len` must be the length that call reported.
void muster_free(const uint8_t *response, size_t len);

#ifdef __cplusplus
}
#endif

#endif  // MUSTER_H
