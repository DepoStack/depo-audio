import { Component, createRef } from 'react'

import { Button } from './ui/button'

export default class AppErrorBoundary extends Component {
  constructor(props) {
    super(props)
    this.state = { failed: false }
    this.fallbackRef = createRef()
  }

  static getDerivedStateFromError() {
    return { failed: true }
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
      <main className="flex min-h-screen items-center justify-center bg-background p-6 text-foreground">
        <section
          ref={this.fallbackRef}
          role="alert"
          tabIndex={-1}
          aria-labelledby="app-error-title"
          className="w-full max-w-lg rounded-xl border border-border bg-card p-6 shadow-lg outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <p className="mb-2 font-mono text-[10px] font-semibold uppercase tracking-widest text-[hsl(var(--gold))]">
            Interface recovery
          </p>
          <h1 id="app-error-title" className="font-serif text-2xl font-semibold">
            DepoAudio needs to reload
          </h1>
          <p className="mt-3 text-sm leading-6 text-[hsl(var(--text2))]">
            The current screen stopped unexpectedly. Reload the interface, then verify any output from a conversion that
            was in progress before trying it again.
          </p>
          <p className="mt-2 text-xs leading-5 text-[hsl(var(--sub))]">
            No diagnostic information is sent automatically.
          </p>
          <Button type="button" variant="primary" size="lg" className="mt-5" onClick={this.reload}>
            Reload DepoAudio
          </Button>
        </section>
      </main>
    )
  }
}
