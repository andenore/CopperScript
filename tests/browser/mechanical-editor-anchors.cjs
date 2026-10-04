/* Reviewed anchor editing on a disposable copy only. */
const assert=require('node:assert/strict'),fs=require('node:fs');
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||'playwright');
(async()=>{
  const [url,source,screenshot]=process.argv.slice(2);assert(source.includes('build'));
  const original=fs.readFileSync(source);
  const browser=await chromium.launch({headless:true,executablePath:process.env.COPPER_BROWSER_EXECUTABLE});
  try {
    const page=await browser.newPage({viewport:{width:1600,height:1100}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll('.component').length===2);
    assert.equal(await page.locator('.datum').count(),2);assert.equal(await page.locator('.named-edge').count(),1);
    await page.locator('#reference').selectOption('R1');await page.locator('#edit-locks').check();
    assert.equal(await page.locator('#prepare-lock').isDisabled(),true);
    assert.match(await page.locator('#notice').innerText(),/attachment DEBUG_PAD/);
    const features=page.locator('#feature');
    const value=await features.locator('option').evaluateAll(options=>options.find(o=>o.textContent.includes('datum CENTER')).value);
    await features.selectOption(value);await page.locator('#feature-param-position').fill('(22mm,15mm)');
    await page.locator('#prepare-feature').click();await page.waitForFunction(()=>!document.querySelector('#save-source').disabled);
    assert.match(await page.locator('#source-diff').innerText(),/22mm/);assert.deepEqual(fs.readFileSync(source),original);
    await page.locator('#save-source').click();await page.waitForFunction(()=>document.querySelector('#source-review').hidden && !document.querySelector('#auto').disabled);
    assert.match(fs.readFileSync(source,'utf8'),/position=\(22mm,15mm\)/);
    assert.equal(await page.locator('.assembly_envelopes').count(),1);
    assert.equal(await page.locator('.assembly_access').count(),1);
    if(screenshot)await page.screenshot({path:screenshot,fullPage:true});
    await page.locator('#undo-source').click();await page.waitForFunction(()=>document.querySelector('#undo-source').disabled && !document.querySelector('#auto').disabled);
    assert.deepEqual(fs.readFileSync(source),original);assert.deepEqual(errors,[]);
    console.log('PASS: datums/edge/anchors rendered, attachment ownership, reviewed dependent-pose save and byte-exact undo; zero page errors');
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
