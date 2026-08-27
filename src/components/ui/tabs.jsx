import * as React from 'react'
import * as TabsPrimitive from '@radix-ui/react-tabs'
import { cn } from '../../lib/utils'

const Tabs = TabsPrimitive.Root

const TabsList = React.forwardRef(({ className, ...props }, ref) => (
  <TabsPrimitive.List
    ref={ref}
    className={cn(
      'inline-flex items-center gap-0.5 rounded-lg bg-[hsl(var(--surface))] border border-border p-[3px]',
      className,
    )}
    {...props}
  />
))
TabsList.displayName = 'TabsList'

const TabsTrigger = React.forwardRef(({ className, ...props }, ref) => (
  <TabsPrimitive.Trigger
    ref={ref}
    className={cn(
      'inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md px-4 py-1.5',
      'border border-transparent text-xs font-semibold text-[hsl(var(--sub))] transition-[color,background-color,border-color,transform] duration-150 active:translate-y-px',
      'hover:text-[hsl(var(--text2))]',
      // Active = gold tint + plum ink. The DepoStack gold is a light warm
      // accent (unreadable as text on light surfaces), so the active state
      // reads through the gold-dim fill + foreground text, not gold text.
      'data-[state=active]:border-primary/35 data-[state=active]:bg-[hsl(var(--gold-dim))] data-[state=active]:text-foreground',
      'focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1',
      className,
    )}
    {...props}
  />
))
TabsTrigger.displayName = 'TabsTrigger'

const TabsContent = React.forwardRef(({ className, ...props }, ref) => (
  <TabsPrimitive.Content
    ref={ref}
    className={cn(
      'flex min-h-0 flex-1 flex-col focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
      className,
    )}
    {...props}
  />
))
TabsContent.displayName = 'TabsContent'

export { Tabs, TabsList, TabsTrigger, TabsContent }
