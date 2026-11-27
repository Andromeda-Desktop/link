// The Link window's connection to andromeda-linkd (crate link-window-ffi): Link1 method calls and signals as raw
// D-Bus bodies (little endian), over the transport link-api picks (session bus, Unix socket or named pipe).
#pragma once
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct LinkConn LinkConn;

typedef enum {
  LINK_UP = 0,     // linkd serves Link1
  LINK_DOWN = 1,   // linkd does not serve Link1; calls fail until the next LINK_UP
  LINK_REPLY = 2,  // the reply to the call with this token
  LINK_ERROR = 3,  // the call with this token failed: error_name, error_message
  LINK_SIGNAL = 4, // a signal at /org/andromeda/Link1: iface, member
} LinkKind;

typedef struct {
  LinkKind kind;
  uint64_t token;
  const char* iface;
  const char* member;
  const char* error_name;
  const char* error_message;
  const char* signature;
  const uint8_t* body;
  size_t len;
  const int* fds; // the receiver owns these and must close them
  size_t nfds;
} LinkEvent;

// Runs on the connection's own thread. The event and its strings and body are valid only during the call.
typedef void (*LinkPost)(void* ctx, const LinkEvent* event);

// Posts only a failure for this call, never its reply.
#define LINK_CALL_ERROR_ONLY 1u

// Starts the connection thread; it reconnects by itself and posts LINK_UP or LINK_DOWN on each change.
LinkConn* link_open(LinkPost post, void* ctx);
// Stops the thread. No post runs after it returns.
void link_close(LinkConn* conn);
// Calls member of iface at /org/andromeda/Link1. The body holds the arguments of the signature. The function dups the
// fds, so the caller keeps its own. timeout_ms 0 means 25 s.
void link_call(LinkConn* conn, uint64_t token, const char* iface, const char* member, const char* signature,
               const uint8_t* body, size_t len, const int* fds, size_t nfds, uint32_t timeout_ms, uint32_t flags);

#ifdef __cplusplus
}
#endif
