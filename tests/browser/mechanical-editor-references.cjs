const assert=require('node:assert/strict'),fs=require('node:fs');
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||'playwright');
(async()=>{
 const [url,source,screenshot]=process.argv.slice(2);assert(source.includes('build'));const original=fs.readFileSync(source);
 const browser=await chromium.launch({headless:true,executablePath:process.env.COPPER_BROWSER_EXECUTABLE});
 try {
  const page=await browser.newPage({viewport:{width:1600,height:1100}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll('.component').length===2);
  const guides=page.locator('.reference-guide');assert.equal(await guides.count(),1);
  assert.equal(await guides.locator('polygon').count(),1);assert.equal(await guides.locator('circle').count(),1);
  assert.equal(await guides.locator('line').count(),1);assert.equal(await guides.locator('path').count(),1);
  assert.equal(await guides.getAttribute('pointer-events'),'none');
  await page.locator('#reference-guides').uncheck();assert.equal(await guides.count(),0);
  await page.locator('#reference-guides').check();assert.equal(await guides.count(),1);
  await page.locator('#side').selectOption('back');assert.equal(await guides.count(),0);
  await page.locator('#side').selectOption('both');assert.equal(await guides.count(),1);
  const features=page.locator('#feature');
  const reference=await features.locator('option').evaluateAll(os=>os.find(o=>o.textContent.includes('reference CASE')).value);
  await features.selectOption(reference);await page.locator('#feature-param-position').fill('(1mm,30mm)');
  await page.locator('#prepare-feature').click();await page.waitForFunction(()=>!document.querySelector('#save-source').disabled);
  assert.deepEqual(fs.readFileSync(source),original);
  await page.locator('#save-source').click();await page.waitForFunction(()=>document.querySelector('#source-review').hidden && !document.querySelector('#auto').disabled);
  assert.match(fs.readFileSync(source,'utf8'),/position=\(1mm,30mm\)/);
  assert.match(await guides.locator('line').getAttribute('x1'),/^11$/);
  await page.locator('#undo-source').click();await page.waitForFunction(()=>!document.querySelector('#auto').disabled && document.querySelector('#undo-source').disabled);
  assert.deepEqual(fs.readFileSync(source),original);
  assert.equal(await page.locator('#feature-param-position').inputValue(),'(0mm,30mm)');
  if(screenshot)await page.screenshot({path:screenshot,fullPage:true});
  assert.deepEqual(errors,[]);
  console.log('PASS: locked guides render all whitelist entities, visibility/side filtering, reviewed transform save and byte-exact undo; zero page errors');
 } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
