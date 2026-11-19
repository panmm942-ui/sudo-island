import { expect, test } from "@playwright/test";

// Live (unfrozen) run of the mock demo script with the mock voice: every state transition,
// content crossfade and the orb pass must run without page errors and end back at compact.
test("demo script runs end to end", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  // Observe before navigation so the production demo can start as soon as the island mounts.
  await page.addInitScript(() => {
    const seen: string[] = ((window as any).__seen = []);
    new MutationObserver(() => {
      const state = document.querySelector(".content")?.getAttribute("data-state");
      if (state && seen.at(-1) !== state) seen.push(state);
    }).observe(document, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["data-state"],
    });
  });
  await page.goto("/?mock=voice&backdrop=fixture&demo=1");
  await page.waitForSelector('body[data-island-ready="1"]', { state: "attached" });
  await page.waitForFunction(
    () => {
      const seen: string[] = (window as any).__seen;
      return seen.includes("responding") && seen.at(-1) === "compact";
    },
    undefined,
    { timeout: 30_000 },
  );
  const seen: string[] = await page.evaluate(() => (window as any).__seen);
  for (const s of ["listening", "thinking", "acting", "responding", "compact"]) expect(seen).toContain(s);
  expect(errors).toEqual([]);
});
