export default function WorkspaceLoading({ label }) {
  return (
    <section
      role="status"
      aria-live="polite"
      aria-label={label}
      className="w-full max-w-[1100px] flex-1 self-center px-5 py-5 sm:px-8"
    >
      <span className="sr-only">{label}</span>
      <div aria-hidden="true" className="animate-pulse motion-reduce:animate-none">
        <div className="mb-5 border-l-2 border-border pl-4">
          <div className="mb-2 h-2 w-24 rounded-full bg-muted" />
          <div className="mb-2 h-6 w-56 max-w-full rounded-md bg-muted" />
          <div className="h-3 w-[28rem] max-w-full rounded-full bg-muted/80" />
        </div>
        <div className="space-y-3">
          <div className="h-24 rounded-xl border border-border/70 bg-card/70" />
          <div className="h-36 rounded-xl border border-border/70 bg-card/70" />
        </div>
      </div>
    </section>
  )
}
