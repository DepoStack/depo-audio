import { useState } from 'react'
import { Copy } from 'lucide-react'

import { Button } from '../ui/button'
import { copyDiagnostics } from '../../lib/diagnostics'

// "Copy diagnostics" (DA-09). The exact text is shown before and after
// copying, so the person can see that it holds no file names or paths.
export default function DiagnosticsCopy({ text, size = 'sm', className = '' }) {
  const [status, setStatus] = useState('')

  const copy = async () => {
    try {
      await copyDiagnostics(text)
      setStatus('copied')
    } catch {
      setStatus('failed')
    }
  }

  return (
    <div className={`flex flex-col gap-2 ${className}`}>
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" size={size} variant="outline" onClick={copy}>
          <Copy size={12} aria-hidden="true" /> Copy diagnostics
        </Button>
        <span role="status" aria-live="polite" aria-atomic="true" className="text-[11px] text-[hsl(var(--text2))]">
          {status === 'copied' && 'Diagnostics copied. Nothing was sent.'}
          {status === 'failed' && "Couldn't copy to the clipboard. Select the text under What is copied and copy it."}
        </span>
      </div>
      <details open={status === 'failed' || undefined}>
        <summary className="w-fit cursor-pointer text-[11px] font-semibold text-[hsl(var(--text2))] focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1">
          What is copied
        </summary>
        <pre className="mt-1.5 select-text whitespace-pre-wrap break-words rounded-md border border-border bg-secondary/40 px-3 py-2 font-mono text-[11px] leading-5 text-foreground">
          {text}
        </pre>
      </details>
    </div>
  )
}
