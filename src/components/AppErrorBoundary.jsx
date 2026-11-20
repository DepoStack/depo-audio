import { Component, createRef } from 'react'

import { Button } from './ui/button'
import DiagnosticsCopy from './common/DiagnosticsCopy'
import { APP_VERSION, buildDiagnostics, errorTypeOf, rememberedEngineInfo } from '../lib/diagnostics'

export default class AppErrorBoundary extends Component {
  constructor(props) {
    super(props)
    this.state = { failed: false, errorType: null }
    this.fallbackRef = createRef()
  }

  // Only the error's class is kept; its message can name files or folders.
  static getDerivedStateFromError(error) {
    return { failed: true, errorType: errorTypeOf(error) }
  }

  componentDidCatch() {
    this.fallbackRef.current?.focus()
  }

  reload = () => {
    if (this.props.onReload) {
      this.props.onReload()
      return
    }

    window.location.reload()
  }

  render() {
    if (!this.state.failed) return this.props.children

    return (
      <main className="flex min-h-[100dvh] items-center justify-center bg-background p-6 text-foreground">
        <section
          ref={this.fallbackRef}
          role="alert"
          tabIndex={-1}
          aria-labelledby="app-error-title"
          className="w-full max-w-lg border border-foreground bg-card p-6 outline-none"
        >
          <p className="mb-2 type-label text-[hsl(var(--sub))]">Interface recovery</p>
          <h1 id="app-error-title" className="type-h3">
            DepoAudio needs to reload
          </h1>
          <p className="mt-1 tabular-nums text-[12px] text-[hsl(var(--sub))]">DepoAudio {APP_VERSION}</p>
          <p className="mt-3 text-sm leading-6 text-[hsl(var(--text2))]">
            The current screen stopped unexpectedly. Reload the interface, then verify any output from a conversion that
            was in progress before trying it again.
          </p>
          <p className="mt-2 type-small leading-5 text-[hsl(var(--sub))]">
            No diagnostic information is sent automatically.
          </p>
          <Button type="button" variant="primary" size="lg" className="mt-5" onClick={this.reload}>
            Reload DepoAudio
          </Button>
          <div className="mt-5 border-t border-border pt-4">
            <p className="mb-2 type-small leading-5 text-[hsl(var(--text2))]">
              If this keeps happening, copy the diagnostics into a GitHub issue. They hold the app version, operating
              system, FFmpeg version and error type, with no file names or folders.
            </p>
            <DiagnosticsCopy text={buildDiagnostics({ ...rememberedEngineInfo(), errorType: this.state.errorType })} />
          </div>
        </section>
      </main>
    )
  }
}
