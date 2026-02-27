import { expect, test } from '@playwright/test'

test('telemetry stream and set_day_speed command work', async ({ page, request }) => {
  const debugApiBase = process.env.DEBUG_API_BASE ?? 'http://127.0.0.1:7777'

  await page.goto('/')
  await expect(page.getByText('World Gen Debug Monitor')).toBeVisible()
  await expect(page.getByText('WS connected')).toBeVisible({ timeout: 15_000 })

  await expect
    .poll(async () => {
      const text = await page.locator('text=/^Frame:/').textContent()
      return text ?? ''
    })
    .not.toContain('Frame: -')

  await page.getByLabel('day speed').fill('0.77')
  await page.getByRole('button', { name: 'Set' }).click()

  await expect
    .poll(async () => {
      const text = await page.locator('text=/^Last ack:/').textContent()
      return text ?? ''
    }, { timeout: 15_000 })
    .toContain('ok')

  const stateResponse = await request.get(`${debugApiBase}/api/state`)
  expect(stateResponse.ok()).toBeTruthy()
  const stateJson = (await stateResponse.json()) as {
    telemetry: { day_speed: number } | null
  }

  expect(stateJson.telemetry).not.toBeNull()
  expect(stateJson.telemetry?.day_speed).toBeCloseTo(0.77, 2)

  await page.screenshot({
    path: '/Users/claus/Repos/world-gen/captures/monitor-playwright.png',
    fullPage: true,
  })
})
