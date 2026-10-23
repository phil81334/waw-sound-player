// XAudio2 is owned by one Rust worker thread. All encoded buffers stay alive
// until their source voice is destroyed; the WebView never owns audio memory.
#include <windows.h>
#include <xaudio2.h>
#include <atomic>
#include <cstdint>
#include <cstring>
#include <new>
#include <vector>

struct VoiceEvents final : IXAudio2VoiceCallback {
    std::atomic<bool> ended{false};
    std::atomic<HRESULT> error{S_OK};
    void __stdcall OnVoiceProcessingPassStart(UINT32) override {}
    void __stdcall OnVoiceProcessingPassEnd() override {}
    void __stdcall OnStreamEnd() override { ended.store(true); }
    void __stdcall OnBufferStart(void*) override {}
    void __stdcall OnBufferEnd(void*) override {}
    void __stdcall OnLoopEnd(void*) override {}
    void __stdcall OnVoiceError(void*, HRESULT code) override { error.store(code); }
};

struct NativeStatus {
    double position;
    std::uint32_t playing;
    std::uint32_t ended;
    std::int32_t error;
};

class Player {
public:
    IXAudio2* engine = nullptr;
    IXAudio2MasteringVoice* master = nullptr;
    IXAudio2SourceVoice* voice = nullptr;
    VoiceEvents events;
    std::vector<std::uint64_t> format;
    std::vector<BYTE> audio;
    std::vector<UINT32> packets;
    UINT32 frames = 0;
    UINT32 blockFrames = 1;
    UINT32 rate = 1;
    UINT32 startFrame = 0;
    bool playing = false;
    bool repeat = false;
    float volume = 0.5f;
    bool com = false;

    Player() { com = SUCCEEDED(CoInitializeEx(nullptr, COINIT_MULTITHREADED)); }
    ~Player() {
        clear();
        if (master) { master->DestroyVoice(); }
        if (engine) { engine->Release(); }
        if (com) { CoUninitialize(); }
    }

    HRESULT initialize() {
        if (engine && master) { return S_OK; }
        if (!engine) {
            HRESULT result = XAudio2Create(&engine, 0, XAUDIO2_DEFAULT_PROCESSOR);
            if (FAILED(result)) { return result; }
        }
        HRESULT result = engine->CreateMasteringVoice(&master);
        if (SUCCEEDED(result)) { master->SetVolume(volume); }
        return result;
    }

    void clear() {
        if (voice) { voice->DestroyVoice(); voice = nullptr; }
        playing = false;
        audio.clear(); packets.clear(); format.clear(); frames = 0;
        events.ended.store(false); events.error.store(S_OK);
    }

    UINT32 position() {
        if (!voice || frames == 0) { return 0; }
        if (events.ended.load()) { return frames; }
        XAUDIO2_VOICE_STATE state{};
        voice->GetState(&state);
        const std::uint64_t current = startFrame + state.SamplesPlayed;
        return repeat ? static_cast<UINT32>(current % frames)
                      : static_cast<UINT32>(current > frames ? frames : current);
    }

    HRESULT queue(UINT32 requested, bool start) {
        if (voice) { voice->DestroyVoice(); voice = nullptr; }
        events.ended.store(false); events.error.store(S_OK);
        playing = false;
        if (audio.empty() || frames == 0) { return E_UNEXPECTED; }
        const UINT32 last = frames > blockFrames ? frames - blockFrames : 0;
        startFrame = (requested > last ? last : requested) / blockFrames * blockFrames;
        HRESULT result = engine->CreateSourceVoice(&voice,
            reinterpret_cast<const WAVEFORMATEX*>(format.data()), 0,
            XAUDIO2_DEFAULT_FREQ_RATIO, &events);
        if (FAILED(result)) { return result; }
        XAUDIO2_BUFFER buffer{};
        buffer.Flags = XAUDIO2_END_OF_STREAM;
        buffer.AudioBytes = static_cast<UINT32>(audio.size());
        buffer.pAudioData = audio.data();
        buffer.PlayBegin = startFrame;
        buffer.PlayLength = startFrame == 0 ? 0 : frames - startFrame;
        buffer.LoopCount = repeat ? XAUDIO2_LOOP_INFINITE : 0;
        XAUDIO2_BUFFER_WMA wma{};
        wma.pDecodedPacketCumulativeBytes = packets.data();
        wma.PacketCount = static_cast<UINT32>(packets.size());
        result = voice->SubmitSourceBuffer(&buffer, packets.empty() ? nullptr : &wma);
        if (FAILED(result)) { voice->DestroyVoice(); voice = nullptr; return result; }
        if (start) {
            result = voice->Start();
            playing = SUCCEEDED(result);
        }
        return result;
    }
};

extern "C" {
void* waw_audio_create() noexcept { return new (std::nothrow) Player(); }
void waw_audio_destroy(void* pointer) noexcept { delete static_cast<Player*>(pointer); }
void waw_audio_clear(void* pointer) noexcept { static_cast<Player*>(pointer)->clear(); }

std::int32_t waw_audio_load(void* pointer, const BYTE* format, UINT32 formatSize,
    const BYTE* data, UINT32 dataSize, const UINT32* packets, UINT32 packetCount,
    UINT32 frames, UINT32 blockFrames, UINT32 rate, bool start) noexcept {
    auto* player = static_cast<Player*>(pointer);
    player->clear();
    HRESULT result = player->initialize();
    if (FAILED(result)) { return result; }
    try {
        player->format.assign((formatSize + 7) / 8 + 1, 0);
        std::memcpy(player->format.data(), format, formatSize);
        player->audio.assign(data, data + dataSize);
        if (packetCount) { player->packets.assign(packets, packets + packetCount); }
        player->frames = frames; player->blockFrames = blockFrames; player->rate = rate;
        result = player->queue(0, start);
    } catch (...) { result = E_OUTOFMEMORY; }
    if (FAILED(result)) { player->clear(); }
    return result;
}

std::int32_t waw_audio_action(void* pointer, UINT32 action, double value) noexcept {
    auto* player = static_cast<Player*>(pointer);
    if (action == 4) {
        if (value < 0 || value > 1) { return E_INVALIDARG; }
        player->volume = static_cast<float>(value);
        return player->master ? player->master->SetVolume(player->volume) : S_OK;
    }
    if (action == 5) {
        const UINT32 position = player->position();
        player->repeat = value != 0;
        return player->voice ? player->queue(position, player->playing) : S_OK;
    }
    if (!player->voice) { return E_UNEXPECTED; }
    switch (action) {
        case 0:
            if (player->events.ended.load()) { return player->queue(0, true); }
            player->playing = true;
            return player->voice->Start();
        case 1:
            player->playing = false;
            return player->voice->Stop();
        case 2: return player->queue(0, false);
        case 3:
            if (value < 0 || value > static_cast<double>(player->frames) / player->rate) { return E_INVALIDARG; }
            return player->queue(static_cast<UINT32>(value * player->rate), player->playing && !player->events.ended.load());
        default: return E_INVALIDARG;
    }
}

NativeStatus waw_audio_status(void* pointer) noexcept {
    auto* player = static_cast<Player*>(pointer);
    const bool ended = player->events.ended.load();
    return { static_cast<double>(player->position()) / player->rate,
        player->playing && !ended ? 1u : 0u, ended ? 1u : 0u, player->events.error.load() };
}
}
