/* global AudioWorkletProcessor, registerProcessor, sampleRate */
// Runs in the AudioWorklet scope. With channelCountMode 'max' and
// 'discrete' interpretation, the input has as many channels as the source
// delivers, so this reports how many channels the web view passes to Web
// Audio. It produces no audio. Reports go out about 4 times a second.

class ChannelCountProbe extends AudioWorkletProcessor {
  constructor() {
    super()
    this.framesSinceReport = 0
  }

  process(inputs) {
    const channels = inputs[0]?.length || 0
    this.framesSinceReport += 128
    if (channels > 0 && this.framesSinceReport >= sampleRate / 4) {
      this.framesSinceReport = 0
      this.port.postMessage({ channels })
    }
    return true
  }
}

registerProcessor('depoaudio-channel-count', ChannelCountProbe)
