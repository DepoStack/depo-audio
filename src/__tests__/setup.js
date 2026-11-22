import '@testing-library/jest-dom/vitest'

// jsdom's window is 1024px wide, which the Player treats as a narrow window
// (side panes become drawers). Tests render at a desktop width by default;
// tests for narrow windows set innerWidth themselves.
Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: 1280 })
