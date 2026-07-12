#include <psmoveapi/psmove.h>
#include <psmoveapi/psmove_tracker.h>

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static uint64_t stable_hash(const char *value)
{
    uint64_t hash = UINT64_C(14695981039346656037);
    while (*value) {
        hash ^= (unsigned char)*value++;
        hash *= UINT64_C(1099511628211);
    }
    return hash ? hash : 1;
}

static void stable_color(const char *serial, unsigned char color[3])
{
    const double hue_sector = (double)(stable_hash(serial) % 360) / 60.0;
    const double chroma = 0.82;
    double sector_mod = hue_sector;
    while (sector_mod >= 2.0) sector_mod -= 2.0;
    const double secondary = chroma * (1.0 - (sector_mod > 1.0 ? sector_mod - 1.0 : 1.0 - sector_mod));
    double red = 0.0, green = 0.0, blue = 0.0;
    switch ((int)hue_sector) {
    case 0: red = chroma; green = secondary; break;
    case 1: red = secondary; green = chroma; break;
    case 2: green = chroma; blue = secondary; break;
    case 3: green = secondary; blue = chroma; break;
    case 4: red = secondary; blue = chroma; break;
    default: red = chroma; blue = secondary; break;
    }
    const double floor = 1.0 - chroma;
    color[0] = (unsigned char)((red + floor) * 255.0 + 0.5);
    color[1] = (unsigned char)((green + floor) * 255.0 + 0.5);
    color[2] = (unsigned char)((blue + floor) * 255.0 + 0.5);
}

int main(int argc, char **argv)
{
    const int camera = argc > 1 ? atoi(argv[1]) : 0;
    const int frames = argc > 2 ? atoi(argv[2]) : 300;
    const float exposure = argc > 3 ? strtof(argv[3], NULL) : 0.12f;
    const int count = psmove_count_connected();
    if (count <= 0) {
        fprintf(stderr, "no PS Move controllers connected\n");
        return 2;
    }

    PSMoveTracker *tracker = psmove_tracker_new_with_camera(camera);
    if (!tracker) {
        fprintf(stderr, "camera %d could not create PSMoveTracker\n", camera);
        return 3;
    }
    psmove_tracker_set_exposure(tracker, exposure);

    PSMove **moves = calloc((size_t)count, sizeof(*moves));
    if (!moves) {
        return 4;
    }
    int enabled = 0;
    for (int index = 0; index < count; index++) {
        PSMove *move = psmove_connect_by_id(index);
        if (!move) {
            continue;
        }
        const char *serial = psmove_get_serial(move);
        unsigned char color[3];
        stable_color(serial, color);
        const enum PSMoveTracker_Status status = psmove_tracker_enable_with_color(
            tracker, move, color[0], color[1], color[2]);
        fprintf(stderr, "controller=%s color=%u,%u,%u calibration=%d\n",
                serial, color[0], color[1], color[2], status);
        if (status == Tracker_CALIBRATED) {
            moves[enabled++] = move;
        } else {
            psmove_disconnect(move);
        }
    }

    const struct PSMoveCameraInfo *info = psmove_tracker_get_camera_info(tracker);
    fprintf(stderr, "camera=%d name=%s api=%s size=%dx%d exposure=%.3f controllers=%d\n",
            camera,
            info && info->camera_name ? info->camera_name : "unknown",
            info && info->camera_api ? info->camera_api : "unknown",
            info ? info->width : 0,
            info ? info->height : 0,
            psmove_tracker_get_exposure(tracker),
            enabled);

    for (int frame = 0; frame < frames; frame++) {
        psmove_tracker_update_image(tracker);
        for (int index = 0; index < enabled; index++) {
            psmove_tracker_update(tracker, moves[index]);
            float x = 0.f, y = 0.f, radius = 0.f;
            const int age_ms = psmove_tracker_get_position(
                tracker, moves[index], &x, &y, &radius);
            if (age_ms >= 0 && radius > 0.f) {
                printf("frame=%d controller=%s x=%.3f y=%.3f radius=%.3f age_ms=%d\n",
                       frame, psmove_get_serial(moves[index]), x, y, radius, age_ms);
                fflush(stdout);
            }
        }
    }

    for (int index = 0; index < enabled; index++) {
        psmove_tracker_disable(tracker, moves[index]);
        psmove_disconnect(moves[index]);
    }
    psmove_tracker_free(tracker);
    free(moves);
    return enabled > 0 ? 0 : 5;
}
