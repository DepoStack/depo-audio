import * as React from 'react'
import * as TabsPrimitive from '@radix-ui/react-tabs'
import { cn } from '../../lib/utils'

const Tabs = TabsPrimitive.Root

const TabsList = React.forwardRef(({ className, ...props }, ref) => (
  <TabsPrimitive.List
    ref={ref}
    className={cn(
      'inline-flex items-center gap-0.5 rounded-lg bg-card border border-border p-[3px]',
      className
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
      'text-xs font-semibold text-[hsl(var(--sub))] transition-colors',
      'hover:text-[hsl(var(--text2))]',
      // gold-hi (not primary): light-mode primary renders 3.34:1 on the active
      // tab background, below the 4.5:1 WCAG AA threshold; gold-hi is 5.2:1
      'data-[state=active]:bg-[hsl(var(--gold-dim))] data-[state=active]:text-[hsl(var(--gold-hi))]',
      'focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring',
      className
    )}
    {...props}
  />
))
TabsTrigger.displayName = 'TabsTrigger'

const TabsContent = React.forwardRef(({ className, ...props }, ref) => (
  <TabsPrimitive.Content
    ref={ref}
    className={cn('contents focus-visible:outline-hidden', className)}
    {...props}
  />
))
TabsContent.displayName = 'TabsContent'

export { Tabs, TabsList, TabsTrigger, TabsContent }
