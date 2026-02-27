import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './tests',
  timeout: 60_000,
  use: {
    baseURL: process.env.MONITOR_BASE_URL ?? 'http://127.0.0.1:4173',
    channel: 'chrome',
    headless: true,
  },
})
