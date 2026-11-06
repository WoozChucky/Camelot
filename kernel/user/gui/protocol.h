/* The Wayland protocol's opcodes the client (gui/client.c) uses, by hand
 * from wayland.xml (Wayland 1.23) and xdg-shell.xml (wayland-protocols
 * 1.38): a request's opcode is its place among its interface's requests,
 * an event's among its events, both from 0. Each comment gives the
 * arguments, in order. The display server (#184) generates its tables from
 * the XML; these are the client's few. */
#ifndef CAMELOT_GUI_PROTOCOL_H
#define CAMELOT_GUI_PROTOCOL_H

/* The versions the client binds, at most (it takes the server's when that
 * is lower): wl_surface.damage_buffer is wl_compositor version 4's. */
#define WL_COMPOSITOR_VERSION 4
#define WL_SHM_VERSION 1
#define XDG_WM_BASE_VERSION 1
#define WL_SEAT_VERSION 1

/* wl_display, always object 1. */
#define WL_DISPLAY_ID 1
#define WL_DISPLAY_SYNC 0         /* new_id wl_callback */
#define WL_DISPLAY_GET_REGISTRY 1 /* new_id wl_registry */
#define WL_DISPLAY_ERROR 0        /* event: object, uint code, string message */
#define WL_DISPLAY_DELETE_ID 1    /* event: uint id */

/* wl_registry */
#define WL_REGISTRY_BIND 0          /* uint name, new_id (string interface, uint version, id) */
#define WL_REGISTRY_GLOBAL 0        /* event: uint name, string interface, uint version */
#define WL_REGISTRY_GLOBAL_REMOVE 1 /* event: uint name */

/* wl_callback */
#define WL_CALLBACK_DONE 0 /* event: uint data */

/* wl_compositor */
#define WL_COMPOSITOR_CREATE_SURFACE 0 /* new_id wl_surface */

/* wl_shm, and its formats (DRM's fourcc codes but for these two). */
#define WL_SHM_CREATE_POOL 0 /* new_id wl_shm_pool, fd, int size */
#define WL_SHM_FORMAT 0      /* event: uint format */
#define WL_SHM_FORMAT_ARGB8888 0
#define WL_SHM_FORMAT_XRGB8888 1

/* wl_shm_pool */
#define WL_SHM_POOL_CREATE_BUFFER 0 /* new_id wl_buffer, int offset, width, height, stride, uint format */
#define WL_SHM_POOL_DESTROY 1

/* wl_buffer */
#define WL_BUFFER_DESTROY 0
#define WL_BUFFER_RELEASE 0 /* event */

/* wl_surface */
#define WL_SURFACE_DESTROY 0
#define WL_SURFACE_ATTACH 1         /* object wl_buffer, int x, int y */
#define WL_SURFACE_DAMAGE 2         /* int x, y, width, height (surface coordinates) */
#define WL_SURFACE_FRAME 3          /* new_id wl_callback */
#define WL_SURFACE_COMMIT 6
#define WL_SURFACE_DAMAGE_BUFFER 9  /* int x, y, width, height (buffer coordinates; version 4) */

/* wl_seat, and its capabilities. */
#define WL_SEAT_GET_POINTER 0  /* new_id wl_pointer */
#define WL_SEAT_GET_KEYBOARD 1 /* new_id wl_keyboard */
#define WL_SEAT_CAPABILITIES 0 /* event: uint capabilities */
#define WL_SEAT_CAPABILITY_POINTER 1
#define WL_SEAT_CAPABILITY_KEYBOARD 2

/* wl_pointer's events. */
#define WL_POINTER_ENTER 0  /* uint serial, object wl_surface, fixed x, fixed y */
#define WL_POINTER_LEAVE 1  /* uint serial, object wl_surface */
#define WL_POINTER_MOTION 2 /* uint time, fixed x, fixed y */
#define WL_POINTER_BUTTON 3 /* uint serial, uint time, uint button, uint state */
#define WL_POINTER_AXIS 4   /* uint time, uint axis, fixed value */
#define WL_POINTER_BUTTON_STATE_PRESSED 1
#define WL_POINTER_AXIS_VERTICAL_SCROLL 0

/* wl_keyboard's events, and its keymap formats. */
#define WL_KEYBOARD_KEYMAP 0    /* uint format, fd, uint size */
#define WL_KEYBOARD_ENTER 1     /* uint serial, object wl_surface, array keys */
#define WL_KEYBOARD_LEAVE 2     /* uint serial, object wl_surface */
#define WL_KEYBOARD_KEY 3       /* uint serial, uint time, uint key, uint state */
#define WL_KEYBOARD_MODIFIERS 4 /* uint serial, depressed, latched, locked, group */
#define WL_KEYBOARD_KEYMAP_FORMAT_NO_KEYMAP 0
#define WL_KEYBOARD_KEY_STATE_PRESSED 1

/* xdg_wm_base */
#define XDG_WM_BASE_DESTROY 0
#define XDG_WM_BASE_GET_XDG_SURFACE 2 /* new_id xdg_surface, object wl_surface */
#define XDG_WM_BASE_PONG 3            /* uint serial */
#define XDG_WM_BASE_PING 0            /* event: uint serial */

/* xdg_surface */
#define XDG_SURFACE_DESTROY 0
#define XDG_SURFACE_GET_TOPLEVEL 1 /* new_id xdg_toplevel */
#define XDG_SURFACE_ACK_CONFIGURE 4 /* uint serial */
#define XDG_SURFACE_CONFIGURE 0    /* event: uint serial */

/* xdg_toplevel */
#define XDG_TOPLEVEL_DESTROY 0
#define XDG_TOPLEVEL_SET_TITLE 2  /* string title */
#define XDG_TOPLEVEL_SET_APP_ID 3 /* string app_id */
#define XDG_TOPLEVEL_MOVE 5       /* object wl_seat, uint serial */
#define XDG_TOPLEVEL_CONFIGURE 0  /* event: int width, int height, array states */
#define XDG_TOPLEVEL_CLOSE 1      /* event */

#endif
