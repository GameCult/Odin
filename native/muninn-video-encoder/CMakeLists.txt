cmake_minimum_required(VERSION 3.24)
project(muninn_video_encoder LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

set(FFMPEG_ROOT "" CACHE PATH "FFmpeg development root containing include/ and lib/")
if(NOT FFMPEG_ROOT)
  message(FATAL_ERROR "Set FFMPEG_ROOT to the OBS/FFmpeg dependency root")
endif()

add_executable(muninn-video-encoder main.cpp)
target_include_directories(muninn-video-encoder PRIVATE "${FFMPEG_ROOT}/include")
target_link_directories(muninn-video-encoder PRIVATE "${FFMPEG_ROOT}/lib")
target_link_libraries(muninn-video-encoder PRIVATE
  avcodec avdevice avfilter avformat avutil)

if(MSVC)
  target_compile_options(muninn-video-encoder PRIVATE /W4 /permissive-)
else()
  target_compile_options(muninn-video-encoder PRIVATE -Wall -Wextra -Wpedantic -Werror)
endif()
