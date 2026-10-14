/* Top Save persists a layout without requiring per-component lock forms. */
const assert=require('node:assert/strict'),fs=require('node:fs');
const {sourceAction}=require('./editor-tools.cjs');
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||'playwright');
(async()=>{
  const [url,source,screenshot]=process.argv.slice(2),original=fs.readFileSync(source);
  assert(source.includes('build'));
  const browser=await chromium.launch({headless:true,executablePath:process.env.COPPER_BROWSER_EXECUTABLE});
  try {
    const page=await browser.newPage({viewport:{width:1600,height:1100}}),errors=[];
    page.on('pageerror',e=>errors.push(e.message));
    await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll('.component').length===4);
    assert(await page.locator('#save-source').isDisabled());
    await page.locator('#auto').click();await page.waitForFunction(()=>!document.querySelector('#apply').disabled);
    assert.match(await page.locator('#save-source').innerText(),/Review placement/);
    await page.locator('#save-source').click();
    await page.waitForFunction(()=>!document.querySelector('#source-review').hidden && !document.querySelector('#save-source').disabled);
    assert.match(await page.locator('#save-help').innerText(),/position, rotation and side locks for 3/);
    assert.match(await page.locator('#source-diff').innerText(),/fixed_placement\(R1\)/);
    assert.deepEqual(fs.readFileSync(source),original);
    await page.locator('#discard-source').click();
    await page.waitForFunction(()=>document.querySelector('#source-review').hidden && !document.querySelector('#apply').disabled);
    await page.locator('#apply').click();
    await page.waitForFunction(()=>!document.querySelector('#auto').disabled);
    assert.match(await page.locator('#source-state').innerText(),/Placement unsaved/);
    const poses=()=>page.locator('.component').evaluateAll(nodes=>nodes.map(n=>[n.dataset.reference,n.getAttribute('transform')]));
    const wanted=await poses();
    await page.locator('#save-source').click();
    await page.waitForFunction(()=>!document.querySelector('#source-review').hidden && !document.querySelector('#save-source').disabled);
    await page.locator('#save-source').click();
    await page.waitForFunction(()=>document.querySelector('#source-review').hidden && !document.querySelector('#undo-source').disabled);
    assert.match(await page.locator('#status').innerText(),/Source saved/);
    assert(!await page.locator('#source-state').innerText().then(s=>s.includes('unsaved')));
    const saved=fs.readFileSync(source);
    assert.equal((saved.toString().match(/constraint fixed_placement\(/g)||[]).length,4);
    await sourceAction(page,'reload-source');
    await page.waitForFunction(()=>!document.querySelector('#reload-source').disabled);
    assert.deepEqual(await poses(),wanted,'Source reopen must reconstruct all saved poses');
    if(screenshot)await page.screenshot({path:screenshot,fullPage:true});
    // Reload intentionally clears source history; restore fixture bytes after verification.
    fs.writeFileSync(source,original);
    assert.deepEqual(errors,[]);
    console.log('PASS: top Save reviews auto-placement, discard retains draft, save creates explicit locks, reload reproduces layout; zero browser errors');
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
