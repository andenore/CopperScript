const assert=require('node:assert/strict'),fs=require('node:fs');
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||'playwright');
(async()=>{
 const [url,source,screenshot]=process.argv.slice(2);assert(source.includes('build'));const original=fs.readFileSync(source);
 const browser=await chromium.launch({headless:true,executablePath:process.env.COPPER_BROWSER_EXECUTABLE});
 try {
  const page=await browser.newPage({viewport:{width:1600,height:1100}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll('.component').length===2);
  assert.equal(await page.locator('path.outline').count(),1);
  assert.equal((await page.locator('path.outline').getAttribute('d')).match(/ A /g).length,4);
  assert.equal(await page.locator('.mechanical-slot').count(),1);
  const features=page.locator('#feature'),outline=await features.locator('option').evaluateAll(os=>os.find(o=>o.textContent.includes('outline rounded_rectangle')).value);
  await features.selectOption(outline);await page.locator('#feature-param-corner_radius').fill('4mm');
  await page.locator('#prepare-feature').click();await page.waitForFunction(()=>!document.querySelector('#save-source').disabled);
  assert.deepEqual(fs.readFileSync(source),original);await page.locator('#save-source').click();
  await page.waitForFunction(()=>document.querySelector('#source-review').hidden && !document.querySelector('#auto').disabled);
  assert.match(fs.readFileSync(source,'utf8'),/corner_radius=4mm/);
  await page.locator('#undo-source').click();await page.waitForFunction(()=>!document.querySelector('#auto').disabled && document.querySelector('#undo-source').disabled);
  assert.deepEqual(fs.readFileSync(source),original);
  await page.locator('summary').filter({hasText:'Closed line/arc path'}).click();
  await page.locator('#prepare-path').click();await page.waitForFunction(()=>!document.querySelector('#save-source').disabled);
  assert.match(await page.locator('#source-diff').innerText(),/boundary TOP line/);
  await page.locator('#save-source').click();await page.waitForFunction(()=>document.querySelector('#source-review').hidden && !document.querySelector('#auto').disabled);
  assert.equal((await page.locator('path.outline').getAttribute('d')).match(/ L /g).length,4);
  if(screenshot)await page.screenshot({path:screenshot,fullPage:true});
  await page.locator('#undo-source').click();await page.waitForFunction(()=>!document.querySelector('#auto').disabled && document.querySelector('#undo-source').disabled);
  assert.deepEqual(fs.readFileSync(source),original);assert.deepEqual(errors,[]);
  console.log('PASS: exact rounded path/slot rendering, reviewed radius save, atomic closed-path creation, byte-exact undo; zero page errors');
 } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
