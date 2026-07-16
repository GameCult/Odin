#include <atomic>
#include <charconv>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <optional>
#include <string>
#include <string_view>
#include <thread>

#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#endif

extern "C" {
#include <libavcodec/avcodec.h>
#include <libavdevice/avdevice.h>
#include <libavformat/avformat.h>
#include <libavutil/avutil.h>
#include <libavutil/error.h>
#include <libavutil/opt.h>
}

namespace {

struct Options {
    std::string input = "ddagrab=framerate=60:output_idx=0:draw_mouse=1";
    int framerate = 60;
    int bitrate_kbps = 12000;
    int gop_frames = 15;
    std::optional<std::uint64_t> max_frames;
    std::optional<std::uint64_t> force_idr_frame;
};

std::string ff_error(int code)
{
    char text[AV_ERROR_MAX_STRING_SIZE]{};
    av_strerror(code, text, sizeof(text));
    return text;
}

[[noreturn]] void fail(const std::string &message)
{
    std::cerr << "muninn-video-encoder: " << message << '\n';
    std::exit(1);
}

int parse_positive(const char *value, const char *name)
{
    char *end = nullptr;
    errno = 0;
    const long parsed = std::strtol(value, &end, 10);
    if (errno != 0 || end == value || *end != '\0' || parsed <= 0 || parsed > INT32_MAX)
        fail(std::string("invalid ") + name);
    return static_cast<int>(parsed);
}

Options parse_options(int argc, char **argv)
{
    Options result;
    for (int index = 1; index < argc; ++index) {
        const std::string_view arg(argv[index]);
        const auto take = [&]() -> const char * {
            if (++index >= argc) fail(std::string("missing value for ") + std::string(arg));
            return argv[index];
        };
        if (arg == "--input") result.input = take();
        else if (arg == "--framerate") result.framerate = parse_positive(take(), "framerate");
        else if (arg == "--bitrate-kbps") result.bitrate_kbps = parse_positive(take(), "bitrate");
        else if (arg == "--gop-frames") result.gop_frames = parse_positive(take(), "gop");
        else if (arg == "--force-idr-frame")
            result.force_idr_frame = static_cast<std::uint64_t>(parse_positive(take(), "force frame"));
        else if (arg == "--frames")
            result.max_frames = static_cast<std::uint64_t>(parse_positive(take(), "frames"));
        else fail("unknown argument: " + std::string(arg));
    }
    return result;
}

struct FormatCloser { void operator()(AVFormatContext *value) const { avformat_close_input(&value); } };
struct CodecCloser { void operator()(AVCodecContext *value) const { avcodec_free_context(&value); } };
struct FrameCloser { void operator()(AVFrame *value) const { av_frame_free(&value); } };
struct PacketCloser { void operator()(AVPacket *value) const { av_packet_free(&value); } };

using FormatPtr = std::unique_ptr<AVFormatContext, FormatCloser>;
using CodecPtr = std::unique_ptr<AVCodecContext, CodecCloser>;
using FramePtr = std::unique_ptr<AVFrame, FrameCloser>;
using PacketPtr = std::unique_ptr<AVPacket, PacketCloser>;

void write_packet(const AVPacket *packet)
{
    if (packet->size == 0) return;
    if (std::fwrite(packet->data, 1, static_cast<size_t>(packet->size), stdout) !=
        static_cast<size_t>(packet->size))
        fail("writing Annex-B stdout failed");
    std::fflush(stdout);
}

void drain_encoder(AVCodecContext *encoder, AVPacket *packet)
{
    while (true) {
        const int result = avcodec_receive_packet(encoder, packet);
        if (result == AVERROR(EAGAIN) || result == AVERROR_EOF) return;
        if (result < 0) fail("receiving encoded packet: " + ff_error(result));
        write_packet(packet);
        av_packet_unref(packet);
    }
}

} // namespace

int main(int argc, char **argv)
{
#ifdef _WIN32
    if (_setmode(_fileno(stdout), _O_BINARY) == -1)
        fail("setting stdout binary mode failed");
#endif
    const Options options = parse_options(argc, argv);
    avdevice_register_all();

    const AVInputFormat *lavfi = av_find_input_format("lavfi");
    if (!lavfi) fail("this FFmpeg build has no lavfi input device");
    AVFormatContext *raw_input = nullptr;
    int result = avformat_open_input(&raw_input, options.input.c_str(), lavfi, nullptr);
    if (result < 0) fail("opening capture input: " + ff_error(result));
    FormatPtr input(raw_input);
    result = avformat_find_stream_info(input.get(), nullptr);
    if (result < 0) fail("reading capture stream information: " + ff_error(result));
    const int video_stream = av_find_best_stream(input.get(), AVMEDIA_TYPE_VIDEO, -1, -1, nullptr, 0);
    if (video_stream < 0) fail("capture input has no video stream");

    const AVCodec *decoder_codec = avcodec_find_decoder(
        input->streams[video_stream]->codecpar->codec_id);
    if (!decoder_codec) fail("capture frame decoder is unavailable");
    CodecPtr decoder(avcodec_alloc_context3(decoder_codec));
    if (!decoder) fail("allocating capture decoder");
    result = avcodec_parameters_to_context(decoder.get(), input->streams[video_stream]->codecpar);
    if (result < 0) fail("copying capture parameters: " + ff_error(result));
    result = avcodec_open2(decoder.get(), decoder_codec, nullptr);
    if (result < 0) fail("opening capture decoder: " + ff_error(result));

    const AVCodec *encoder_codec = avcodec_find_encoder_by_name("h264_nvenc");
    if (!encoder_codec) fail("h264_nvenc is unavailable");
    CodecPtr encoder(avcodec_alloc_context3(encoder_codec));
    if (!encoder) fail("allocating h264_nvenc context");

    PacketPtr input_packet(av_packet_alloc());
    PacketPtr output_packet(av_packet_alloc());
    FramePtr frame(av_frame_alloc());
    if (!input_packet || !output_packet || !frame) fail("allocating frame pipeline");

    std::atomic_bool force_next_idr{false};
    std::atomic_uint64_t requested_bitrate_kbps{0};
    std::atomic_bool quit{false};
    std::thread command_reader([&] {
        std::string command;
        while (std::getline(std::cin, command)) {
            if (command == "IDR") force_next_idr.store(true, std::memory_order_release);
            else if (command.starts_with("BITRATE ")) {
                std::uint64_t bitrate = 0;
                const auto value = std::string_view(command).substr(8);
                const auto parsed = std::from_chars(value.data(), value.data() + value.size(), bitrate);
                if (parsed.ec == std::errc{} && parsed.ptr == value.data() + value.size() &&
                    bitrate >= 250 && bitrate <= 100'000)
                    requested_bitrate_kbps.store(bitrate, std::memory_order_release);
            }
            else if (command == "QUIT") { quit.store(true, std::memory_order_release); break; }
        }
    });
    command_reader.detach();

    bool encoder_open = false;
    std::uint64_t frame_number = 0;
    while (!quit.load(std::memory_order_acquire)) {
        result = av_read_frame(input.get(), input_packet.get());
        if (result == AVERROR_EOF) break;
        if (result < 0) fail("reading capture frame: " + ff_error(result));
        if (input_packet->stream_index != video_stream) {
            av_packet_unref(input_packet.get());
            continue;
        }
        result = avcodec_send_packet(decoder.get(), input_packet.get());
        av_packet_unref(input_packet.get());
        if (result < 0) fail("submitting capture packet: " + ff_error(result));

        while ((result = avcodec_receive_frame(decoder.get(), frame.get())) >= 0) {
            if (!encoder_open) {
                encoder->width = frame->width;
                encoder->height = frame->height;
                encoder->pix_fmt = static_cast<AVPixelFormat>(frame->format);
                encoder->time_base = AVRational{1, options.framerate};
                encoder->framerate = AVRational{options.framerate, 1};
                encoder->bit_rate = static_cast<int64_t>(options.bitrate_kbps) * 1000;
                encoder->rc_max_rate = encoder->bit_rate;
                encoder->rc_buffer_size = static_cast<int>(encoder->bit_rate / options.framerate * 2);
                encoder->gop_size = options.gop_frames;
                encoder->max_b_frames = 0;
                if (frame->hw_frames_ctx) encoder->hw_frames_ctx = av_buffer_ref(frame->hw_frames_ctx);
                AVDictionary *settings = nullptr;
                av_dict_set(&settings, "preset", "p1", 0);
                av_dict_set(&settings, "tune", "ull", 0);
                av_dict_set(&settings, "zerolatency", "1", 0);
                av_dict_set(&settings, "delay", "0", 0);
                av_dict_set(&settings, "rc", "cbr", 0);
                av_dict_set(&settings, "rc-lookahead", "0", 0);
                av_dict_set(&settings, "multipass", "disabled", 0);
                av_dict_set(&settings, "strict_gop", "1", 0);
                av_dict_set(&settings, "forced-idr", "1", 0);
                av_dict_set(&settings, "aud", "1", 0);
                result = avcodec_open2(encoder.get(), encoder_codec, &settings);
                av_dict_free(&settings);
                if (result < 0) fail("opening h264_nvenc: " + ff_error(result));
                if (encoder->extradata && encoder->extradata_size > 0) {
                    if (std::fwrite(encoder->extradata, 1,
                                    static_cast<size_t>(encoder->extradata_size), stdout) !=
                        static_cast<size_t>(encoder->extradata_size))
                        fail("writing encoder headers failed");
                }
                std::cerr << "muninn-video-encoder: " << encoder->width << 'x'
                          << encoder->height << " format=" << encoder->pix_fmt
                          << " headers=" << encoder->extradata_size << '\n';
                encoder_open = true;
            }

            const bool scheduled = options.force_idr_frame == frame_number;
            const std::uint64_t requested_bitrate =
                requested_bitrate_kbps.exchange(0, std::memory_order_acq_rel);
            if (requested_bitrate > 0 &&
                encoder->bit_rate != static_cast<int64_t>(requested_bitrate * 1000)) {
                encoder->bit_rate = static_cast<int64_t>(requested_bitrate * 1000);
                encoder->rc_max_rate = encoder->bit_rate;
                encoder->rc_buffer_size = static_cast<int>(
                    encoder->bit_rate / options.framerate * 2);
                std::cerr << "muninn-video-encoder: bitrate=" << requested_bitrate
                          << "kbps frame=" << frame_number << '\n';
            }
            if (scheduled || force_next_idr.exchange(false, std::memory_order_acq_rel))
                frame->pict_type = AV_PICTURE_TYPE_I;
            else
                frame->pict_type = AV_PICTURE_TYPE_NONE;
            frame->pts = static_cast<int64_t>(frame_number++);
            result = avcodec_send_frame(encoder.get(), frame.get());
            av_frame_unref(frame.get());
            if (result < 0) fail("submitting frame to h264_nvenc: " + ff_error(result));
            drain_encoder(encoder.get(), output_packet.get());
            if (options.max_frames && frame_number >= *options.max_frames) {
                quit.store(true, std::memory_order_release);
                break;
            }
        }
        if (quit.load(std::memory_order_acquire)) break;
        if (result != AVERROR(EAGAIN) && result != AVERROR_EOF)
            fail("decoding capture frame: " + ff_error(result));
    }

    if (encoder_open) {
        result = avcodec_send_frame(encoder.get(), nullptr);
        if (result >= 0) drain_encoder(encoder.get(), output_packet.get());
    }
    return 0;
}
