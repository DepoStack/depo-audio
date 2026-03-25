import { Loader2 } from 'lucide-react'

export default function Spinner({ className }) {
  return <Loader2 className={`animate-spin inline-block ${className || 'h-3.5 w-3.5'}`} />
}
