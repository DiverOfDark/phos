import { chromium } from 'playwright';

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto('http://localhost:5173/');
  await page.waitForTimeout(2000); // Wait for animations
  await page.screenshot({ path: '/root/.openclaw/workspace/phos_dashboard.png', fullPage: true });
  await browser.close();
  console.log('Screenshot saved to /root/.openclaw/workspace/phos_dashboard.png');
})();
