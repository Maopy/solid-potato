const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
let chromium;
try { ({chromium} = require(process.env.PLAYWRIGHT_MODULE || 'playwright')); } catch (_) {}

test('offline Douyin page uses the normal panel and reports failure only on action', {skip: !chromium}, async () => {
  const browser = await chromium.launch({channel:process.env.PLAYWRIGHT_CHANNEL || 'msedge',headless:true});
  try {
    const page = await browser.newPage();
    let script = fs.readFileSync('assets/moderation/userscript/douyin.user.js','utf8')
      .replace('__BASE_JSON__','"http://127.0.0.1:2025"').replace('__KEY_JSON__','"fixture"').replace('__CLIENT_VERSION_JSON__','"0.4.8"');
    for (const [key,path] of Object.entries({SDK:'sdk/probe',RAW:'sdk/raw',SUBMIT:'sdk/submit',DECODER:'adapters/douyin-decoder',ADAPTER:'adapters/douyin-live',FILL:'adapters/douyin-fill',RECORDING:'adapters/douyin-recording',OFFLINE_PANEL:'sdk/panel',PANEL:'sdk/panel'})) {
      script = script.replace('/*__'+key+'__*/',fs.readFileSync('assets/moderation/'+path+'.js','utf8'));
    }
    await page.route('**/*', route => route.fulfill({status:200,contentType:'text/html',body:'<!doctype html><html><body>fixture</body></html>'}));
    await page.addInitScript(() => { window.GM_xmlhttpRequest = request => setTimeout(() => request.onerror(), 0); });
    await page.addInitScript({content:script});
    await page.goto('https://live.douyin.com/123');
    const panel = page.locator('#live-archive-assistant');
    await panel.waitFor();
    assert.equal(await page.locator('#live-archive-connecting').count(),0);
    assert.equal(await page.getByText(/无法连接本机中控/).count(),0);
    await panel.locator('button[aria-expanded]').click();
    await page.locator('#live-archive-moderation-toggle').click();
    assert.equal(await page.getByText(/无法连接本机场控/).count(),1);
    await page.locator('#live-archive-like-toggle').click();
    assert.equal(await page.getByText(/无法连接本机场控/).count(),1);
    await page.locator('#live-archive-speech-toggle').click();
    assert.equal(await page.getByText(/无法连接本机场控/).count(),1);
  } finally { await browser.close(); }
});
