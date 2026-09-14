#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Global origin uses Core Graphics points (top-left of the primary display).
// Image dimensions use backing pixels; never scale a global origin by Retina DPI.
typedef struct {
    double x, y, scale;
    uint32_t width, height, display_id;
} PulseMonitor;

bool pulse_capture_supported(void);
bool pulse_capture_allowed(void);
bool pulse_request_capture_access(void);
bool pulse_monitor_at_pointer(PulseMonitor *monitor, char **error);
void pulse_configure_overlay(void *window);
bool pulse_position_overlay(void *window, PulseMonitor monitor, bool notice, char **error);
bool pulse_capture_screen(PulseMonitor monitor, uint8_t **pixels, char **error);
char *pulse_recognize_text(const uint8_t *rgba, uint32_t width, uint32_t height, char **error);
bool pulse_prepare_text_recognition(char **error);
void pulse_native_free(void *pointer);
