/* Invalid geometry can be corrected by dragging/rotation, without snap-back. */
const assert=require('node:assert/strict'),fs=require('node:fs');
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||'playwright');
(async()=>{
  const [url,source,screenshot]=process.argv.slice(2),original=fs.readFileSync(source);
  const browser=await chromium.launch({headless:true,executablePath:process.env.COPPER_BROWSER_EXECUTABLE});
  try {
    const page=await browser.newPage({viewport:{width:1500,height:1100}}),errors=[];
    page.on('pageerror',e=>errors.push(e.message));
    await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll('.component').length===4);
    await page.locator('#auto').click();await page.waitForFunction(()=>!document.querySelector('#apply').disabled);
    await page.locator('#apply').click();await page.waitForFunction(()=>!document.querySelector('#auto').disabled);
    await page.locator('#reference').selectOption('R1');
    await page.locator('#x').fill('20');await page.locator('#y').fill('12');await page.locator('#rotation').fill('0');
    await page.locator('#move').click();await page.waitForFunction(()=>!document.querySelector('#apply').disabled);
    await page.locator('#apply').click();await page.waitForFunction(()=>!document.querySelector('#auto').disabled);
    const screenPoint=async(x,y)=>page.locator('#board').evaluate((svg,p)=>{
      const point=new DOMPoint(...p).matrixTransform(svg.getScreenCTM());return {x:point.x,y:point.y};
    },[x,y]);
    const origin=await screenPoint(20,12),outside=await screenPoint(42,12);
    await page.mouse.move(origin.x,origin.y);await page.mouse.down();
    await page.mouse.move(outside.x,outside.y,{steps:8});await page.mouse.up();
    await page.waitForFunction(()=>document.querySelector('#x').value==='42' && !document.querySelector('#move').disabled);
    assert(await page.locator('[data-reference="R1"]').evaluate(n=>n.classList.contains('pose-invalid')));
    assert(await page.locator('#pose-validation').isVisible());
    assert.match(await page.locator('#pose-validation-reasons').textContent(),/board material/);
    assert(await page.locator('#apply').isDisabled());
    await page.locator('#rotate-right').click();
    await page.waitForFunction(()=>document.querySelector('#rotation').value==='45'&&!document.querySelector('#move').disabled);
    assert.equal(await page.locator('#x').inputValue(),'42');
    await page.locator('#board').focus();await page.keyboard.press('Shift+r');
    await page.waitForFunction(()=>document.querySelector('#rotation').value==='0'&&!document.querySelector('#move').disabled);
    await page.keyboard.press('ArrowLeft');
    await page.waitForFunction(()=>document.querySelector('#x').value==='41.9'&&!document.querySelector('#move').disabled);
    if(screenshot)await page.screenshot({path:screenshot,fullPage:true});
    // Drag the pending pose back inside; default mode applies the legal result.
    const start=await screenPoint(41.9,12),end=await screenPoint(20,12);
    await page.mouse.move(start.x,start.y);await page.mouse.down();
    await page.mouse.move(end.x,end.y,{steps:8});await page.mouse.up();
    await page.waitForFunction(()=>document.querySelector('#x').value==='20'&&!document.querySelector('#auto').disabled);
    assert(!await page.locator('#pending').isVisible());
    assert(!await page.locator('[data-reference="R1"]').evaluate(n=>n.classList.contains('pose-invalid')));
    // Invalid numeric edits also remain visible, and Escape restores accepted pose.
    await page.locator('#x').fill('42');await page.locator('#move').click();
    await page.waitForFunction(()=>document.querySelector('#x').value==='42'&&!document.querySelector('#move').disabled);
    await page.locator('#board').focus();await page.keyboard.press('Escape');
    await page.waitForFunction(()=>document.querySelector('#x').value==='20'&&!document.querySelector('#auto').disabled);
    assert.deepEqual(fs.readFileSync(source),original);
    assert.deepEqual(errors,[]);console.log('PASS: outside-board draft remains red and editable; buttons, R/Shift+R, nudging, corrective drag and Escape; source unchanged');
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
