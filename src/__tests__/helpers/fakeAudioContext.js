import { vi } from 'vitest'

// A small stand-in for Web Audio: nodes remember what they are connected to,
// gain params remember their target value, and every context is kept so tests
// can check that it was closed.
export function installFakeAudio() {
  const contexts = []
  const workletNodes = []

  class FakeParam {
    constructor(value) {
      this.value = value
    }
    setTargetAtTime(value) {
      this.value = value
    }
    cancelScheduledValues() {}
  }

  class FakeNode {
    constructor(context, kind, extra = {}) {
      this.context = context
      this.kind = kind
      this.connections = []
      Object.assign(this, extra)
      context.nodes.push(this)
    }
    connect(target, output = 0, input = 0) {
      this.connections.push({ target, output, input })
      return target
    }
    disconnect(target) {
      this.connections = target ? this.connections.filter(c => c.target !== target) : []
    }
  }

  class FakeAudioContext {
    constructor() {
      this.nodes = []
      this.state = 'suspended'
      this.currentTime = 0
      this.closed = false
      this.destination = new FakeNode(this, 'destination')
      this.audioWorklet = { addModule: vi.fn(() => Promise.resolve()) }
      this.resume = vi.fn(() => {
        this.state = 'running'
        return Promise.resolve()
      })
      this.close = vi.fn(() => {
        this.closed = true
        this.state = 'closed'
        return Promise.resolve()
      })
      contexts.push(this)
    }
    createMediaElementSource(element) {
      if (element.__hasSource) throw new Error('InvalidStateError: element already has a source')
      element.__hasSource = true
      return new FakeNode(this, 'source', { element })
    }
    createGain() {
      return new FakeNode(this, 'gain', { gain: new FakeParam(1) })
    }
    createChannelSplitter(count) {
      return new FakeNode(this, 'splitter', { numberOfOutputs: count })
    }
    createChannelMerger(count) {
      return new FakeNode(this, 'merger', { numberOfInputs: count })
    }
  }

  class FakeAudioWorkletNode extends FakeNode {
    constructor(context, name, options) {
      super(context, 'worklet', { name, options })
      this.port = { onmessage: null, close: vi.fn() }
      workletNodes.push(this)
    }
  }

  const previous = {
    AudioContext: globalThis.AudioContext,
    AudioWorkletNode: globalThis.AudioWorkletNode,
  }
  globalThis.AudioContext = FakeAudioContext
  globalThis.AudioWorkletNode = FakeAudioWorkletNode
  window.AudioContext = FakeAudioContext

  return {
    FakeAudioContext,
    contexts,
    workletNodes,
    // Per-channel GainNodes of a context, in channel order.
    channelGains(context) {
      const splitter = context.nodes.find(n => n.kind === 'splitter' && n.connections.length > 0)
      if (!splitter) return []
      return splitter.connections
        .slice()
        .sort((a, b) => a.output - b.output)
        .map(c => c.target)
    },
    master(context) {
      return context.nodes.find(n => n.kind === 'gain' && n.connections.some(c => c.target === context.destination))
    },
    restore() {
      globalThis.AudioContext = previous.AudioContext
      globalThis.AudioWorkletNode = previous.AudioWorkletNode
      window.AudioContext = previous.AudioContext
    },
  }
}
