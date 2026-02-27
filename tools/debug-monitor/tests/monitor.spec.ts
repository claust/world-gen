import { expect, test } from "@playwright/test";

test("telemetry stream, day speed, and WASD movement commands work", async ({ page, request }) => {
  const debugApiBase = process.env.DEBUG_API_BASE ?? "http://127.0.0.1:7777";

  await page.goto("/");
  await expect(page.getByText("World Gen Debug Monitor")).toBeVisible();
  await expect(page.getByText("WS connected")).toBeVisible({ timeout: 15_000 });

  await expect
    .poll(async () => {
      const text = await page.locator("text=/^Frame:/").textContent();
      return text ?? "";
    })
    .not.toContain("Frame: -");

  await page.getByLabel("day speed").fill("0.77");
  await page.getByRole("button", { name: "Set" }).click();

  await expect
    .poll(
      async () => {
        const text = await page.locator("text=/^Last ack:/").textContent();
        return text ?? "";
      },
      { timeout: 15_000 },
    )
    .toContain("ok");

  await expect
    .poll(
      async () => {
        const stateResponse = await request.get(`${debugApiBase}/api/state`);
        if (!stateResponse.ok()) return 0;
        const stateJson = (await stateResponse.json()) as {
          telemetry: { day_speed: number } | null;
        };
        return stateJson.telemetry?.day_speed ?? 0;
      },
      { timeout: 15_000 },
    )
    .toBeCloseTo(0.77, 2);

  const initialStateResponse = await request.get(`${debugApiBase}/api/state`);
  expect(initialStateResponse.ok()).toBeTruthy();
  const initialStateJson = (await initialStateResponse.json()) as {
    telemetry: {
      camera: { x: number; y: number; z: number };
    } | null;
  };
  const initialCamera = initialStateJson.telemetry?.camera;
  expect(initialCamera).toBeDefined();

  await page.locator("main").click();
  await page.keyboard.down("KeyW");
  await page.waitForTimeout(250);
  await page.keyboard.up("KeyW");

  await expect
    .poll(
      async () => {
        const text = await page.locator("text=/^Last ack:/").textContent();
        return text ?? "";
      },
      { timeout: 15_000 },
    )
    .toContain("move key w released");

  await expect
    .poll(
      async () => {
        const response = await request.get(`${debugApiBase}/api/state`);
        if (!response.ok()) return 0;
        const json = (await response.json()) as {
          telemetry: {
            camera: { x: number; y: number; z: number };
          } | null;
        };
        if (!json.telemetry || !initialCamera) return 0;

        const dx = json.telemetry.camera.x - initialCamera.x;
        const dy = json.telemetry.camera.y - initialCamera.y;
        const dz = json.telemetry.camera.z - initialCamera.z;
        return Math.hypot(dx, dy, dz);
      },
      { timeout: 15_000 },
    )
    .toBeGreaterThan(0.5);

  await page.screenshot({
    path: "/Users/claus/Repos/world-gen/captures/monitor-playwright.png",
    fullPage: true,
  });
});
