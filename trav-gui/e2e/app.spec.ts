import fs from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { LAB, readEnv } from "./env";

test.describe.configure({ mode: "serial" });

const env = readEnv();
const album = env.torrents.find((t) => t.name === "Album")!;
const notes = env.torrents.find((t) => t.name === "notes.iso")!;
const seedPeer = `127.0.0.1:${env.seedPort}`;

let errors: string[] = [];
test.beforeEach(async ({ page }) => {
  errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
});
test.afterEach(() => expect(errors, "browser console errors").toEqual([]));

const row = (page: Page, name: string) => page.locator(".trow", { has: page.locator(".tname", { hasText: name }) });

/** Load the UI and wait until it is mounted and talking to the daemon. */
async function openApp(page: Page, url = "/") {
  await page.goto(url);
  await expect(page.locator(".statusbar")).toContainText("daemon");
}

async function rpc(page: Page, method: string, params?: unknown) {
  const r = await page.request.post("/api/rpc", { data: { method, params: params ?? null } });
  expect(r.ok()).toBeTruthy();
  return (await r.json()).result;
}

test("empty library", async ({ page }) => {
  await openApp(page);
  await expect(page.getByRole("heading", { name: "Nothing downloading." })).toBeVisible();
  await expect(page.locator(".statusbar")).toContainText("daemon");
});

test("add a .torrent, skip a file, download, then re-enable it", async ({ page }) => {
  await openApp(page);
  const chooser = page.waitForEvent("filechooser");
  await page.locator(".tb-actions .btn.primary").click();
  await (await chooser).setFiles(album.path);

  const dialog = page.getByRole("dialog", { name: "Add torrent" });
  await expect(dialog.locator(".modal-title")).toHaveText("Album");
  await expect(dialog.locator(".file-row")).toHaveCount(3);
  await dialog.locator(".file-row", { hasText: "02 - Demo.flac" }).locator("input").uncheck();
  await dialog.getByRole("button", { name: "Add torrent" }).click();
  await expect(dialog).toBeHidden();

  await expect(row(page, "Album")).toBeVisible();
  await rpc(page, "addPeers", { hash: album.hash, peers: [seedPeer] });
  await expect(row(page, "Album").locator(".slabel")).toHaveText("Seeding", { timeout: 30_000 });

  await row(page, "Album").click();
  await page.keyboard.press("2");
  const demo = page.locator(".files .file-row", { hasText: "02 - Demo.flac" });
  await expect(demo).toHaveAttribute("data-skip", "true");
  expect(fs.existsSync(path.join(LAB, "dl", "Album", "01 - Opening.flac"))).toBe(true);

  await demo.locator("input").check();
  await expect(page.locator(".files .file-prog .mono", { hasText: "100%" })).toHaveCount(3, { timeout: 30_000 });
  const got = fs.readFileSync(path.join(LAB, "dl", "Album", "Bonus", "02 - Demo.flac"));
  const want = fs.readFileSync(path.join(LAB, "content", "Album", "Bonus", "02 - Demo.flac"));
  expect(got.equals(want)).toBe(true);
});

test("paste a magnet anywhere; metadata arrives from the swarm", async ({ page }) => {
  await openApp(page);
  await expect(row(page, "Album")).toBeVisible();
  const magnet = `magnet:?xt=urn:btih:${notes.hash}&dn=notes.iso&x.pe=${seedPeer}`;
  await page.evaluate((m) => {
    const dt = new DataTransfer();
    dt.setData("text", m);
    // Browsers target the focused element (body when nothing is focused) and bubble.
    document.body.dispatchEvent(new ClipboardEvent("paste", { clipboardData: dt, bubbles: true }));
  }, magnet);
  const dialog = page.getByRole("dialog", { name: "Add torrent" });
  await expect(dialog).toContainText("files can be chosen once metadata arrives");
  await dialog.getByRole("button", { name: "Add torrent" }).click();
  await expect(row(page, "notes.iso").locator(".slabel")).toHaveText("Seeding", { timeout: 30_000 });
  await expect(page.locator(".toast", { hasText: "Metadata received" })).toBeVisible();
});

test("context menu pause / resume and keyboard selection", async ({ page }) => {
  await openApp(page);
  const r = row(page, "notes.iso");
  await r.click({ button: "right" });
  await page.getByRole("menuitem", { name: "Pause" }).click();
  await expect(r.locator(".slabel")).toHaveText("Paused");
  await r.click();
  await page.keyboard.press("Space");
  await expect(r.locator(".slabel")).toHaveText("Seeding");
  await page.keyboard.press("ArrowUp");
  await expect(row(page, "Album")).toHaveAttribute("data-focused", "true");
});

test("command palette and themes persist", async ({ page }) => {
  await openApp(page);
  await expect(row(page, "Album")).toBeVisible();
  await page.keyboard.press("Control+k");
  await expect(page.locator(".palette-input input")).toBeFocused();
  await page.keyboard.type("pause all");
  await page.keyboard.press("Enter");
  await expect(page.locator(".trow .slabel", { hasText: "Paused" })).toHaveCount(2);
  await page.keyboard.press("Control+k");
  await expect(page.locator(".palette-input input")).toBeFocused();
  await expect(page.locator(".palette-input input")).toHaveValue("");
  await page.keyboard.type("resume all");
  await page.keyboard.press("Enter");
  await expect(page.locator(".trow .slabel", { hasText: "Paused" })).toHaveCount(0);

  await page.keyboard.press("Control+Shift+L");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "paper");
  await page.reload();
  // The pre-paint boot script applies the saved theme before the app mounts.
  await expect(page.locator("html")).toHaveAttribute("data-theme", "paper");
  await expect(row(page, "Album")).toBeVisible();
  await page.keyboard.press("Control+Shift+L");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "phosphor");
  await page.keyboard.press("Control+Shift+L");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "ink");
});

test("settings save to the engine", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Control+,");
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByRole("button", { name: "Bandwidth" }).click();
  await dialog.locator(".set-row", { hasText: "Download limit" }).locator("input").fill("512");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(page.locator(".toast", { hasText: "Settings saved" })).toBeVisible();
  await expect(page.locator(".statusbar")).toContainText("/ 512 KB/s");
  expect((await rpc(page, "getSettings")).downloadLimit).toBe(512 * 1024);
  await rpc(page, "setSettings", { ...(await rpc(page, "getSettings")), downloadLimit: 0 });
});

test("remove with data deletes files", async ({ page }) => {
  await openApp(page);
  await row(page, "Album").click();
  await page.keyboard.press("Delete");
  const dialog = page.getByRole("dialog", { name: "Remove torrents" });
  await dialog.getByText("Also delete downloaded files").click();
  await expect(dialog.getByRole("switch")).toBeChecked();
  await dialog.getByRole("button", { name: "Delete" }).click();
  await expect(row(page, "Album")).toHaveCount(0);
  await expect.poll(() => fs.existsSync(path.join(LAB, "dl", "Album"))).toBe(false);
});

test("token-locked daemon asks for the token", async ({ page }) => {
  await page.goto(env.lockedUrl);
  await expect(page.getByRole("heading", { name: "Enter access token" })).toBeVisible();
  await page.locator(".gate-card input").fill(env.token);
  await page.getByRole("button", { name: "Unlock" }).click();
  await expect(page.getByRole("heading", { name: "Nothing downloading." })).toBeVisible();
  errors = errors.filter((e) => !e.includes("401")); // the initial locked request
});
