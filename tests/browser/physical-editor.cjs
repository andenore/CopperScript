/* Reviewed physical feature edits in both board and definition documents. */
const assert=require('node:assert/strict'), fs=require('node:fs');
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||'playwright');
(async()=>{
  const [url,source,kind,screenshot]=process.argv.slice(2), original=fs.readFileSync(source);
  const browser=await chromium.launch({headless:true,executablePath:process.env.COPPER_BROWSER_EXECUTABLE});
  try {
    const page=await browser.newPage({viewport:{width:1500,height:1000}}),errors=[];
    page.on('pageerror',e=>errors.push(e.message));
    await page.goto(url);
    await page.waitForFunction(()=>document.querySelector('#source-state').textContent==='Source ready');
    await page.locator('#tab-features').click();
    const choose=async(pattern)=>{
      const value=await page.locator('#physical-feature option').evaluateAll((options,p)=>options.find(o=>o.textContent.includes(p))?.value,pattern);
      assert.notEqual(value,undefined,pattern);await page.locator('#physical-feature').selectOption(value);
    };
    const save=async(staged=false)=>{
      await page.locator(staged?'#physical-review-staged':'#physical-review').click();
      await page.waitForFunction(()=>!document.querySelector('#save-source').disabled);
      assert.match(await page.locator('#source-diff').textContent(),/proposed.copper/);
      assert.deepEqual(fs.readFileSync(source),original,'review cannot write source');
      await page.locator('#save-source').click();
      await page.waitForFunction(()=>document.querySelector('#source-state').textContent==='Source ready');
      assert.notDeepEqual(fs.readFileSync(source),original);
    };
    if(kind==='board') {
      const zone=await page.locator('#physical-kind option').evaluateAll(options=>options.find(o=>o.textContent==='constraint: copper_zone').value);
      await page.locator('#physical-kind').selectOption(zone);
      await page.locator('#physical-targets').fill('A');await page.locator('#physical-polygon').click();
      for(const [x,y] of [[1,1],[30,1],[30,25],[1,25]]) {
        const point=await page.locator('#board').evaluate((svg,p)=>{const q=new DOMPoint(...p).matrixTransform(svg.getScreenCTM());return {x:q.x,y:q.y};},[x,y]);
        await page.mouse.click(point.x,point.y);
      }
      await page.locator('#physical-review').click();
      await page.waitForFunction(()=>!document.querySelector('#save-source').disabled);
      assert.equal(await page.locator('.physical-copper_zone').count(),1);
      const discarded=page.waitForResponse(r=>r.url().endsWith('/api/operation')&&r.request().postDataJSON()?.action==='discard_source');
      await page.locator('#discard-source').click();await discarded;
      await page.waitForFunction(()=>document.querySelector('#source-state').textContent==='Source ready');
      await page.locator('#physical-pick-point').uncheck();
      assert.deepEqual(fs.readFileSync(source),original);
      await choose('artwork: text LABEL');
      await page.locator('#physical-param-text').fill('"Reviewed board"');
      await page.locator('#physical-stage').click();
      await choose('constraint: allowed_orientations');
      await page.locator('#physical-param-values').fill('"0,90,180"');
      await page.locator('#physical-stage').click();await save(true);
      assert.match(fs.readFileSync(source,'utf8'),/Reviewed board/);
      assert.equal(await page.locator('#physical-stackup .stackup-layer').count(),3);
    } else if(kind==='macro') {
      const box=await page.locator('.component[data-reference="passive"] .pad').first().boundingBox();
      await page.mouse.move(box.x+box.width/2,box.y+box.height/2);await page.mouse.down();
      await page.mouse.move(box.x+box.width/2+15,box.y+box.height/2,{steps:5});await page.mouse.up();
      await page.waitForFunction(()=>!document.querySelector('#save-source').disabled,{},{timeout:5000}).catch(async error=>{
        throw new Error(`${error.message}; status=${await page.locator('#status').textContent()}; page errors=${errors.join('; ')}`);
      });
      assert.deepEqual(fs.readFileSync(source),original,'drag is reviewed before saving');
      const discarded=page.waitForResponse(r=>r.url().endsWith('/api/operation')&&r.request().postDataJSON()?.action==='discard_source');
      await page.locator('#discard-source').click();await discarded;
      await page.waitForFunction(()=>document.querySelector('#source-state').textContent==='Source ready');
      await choose('macro: member passive');
      await page.locator('#physical-param-at').fill('(3 mm, 0 mm)');await save();
      assert.match(fs.readFileSync(source,'utf8'),/3 mm, 0 mm/);
      assert.equal(await page.locator('.physical-track').count()>0,true);
    } else {
      await choose('profile: hole H1');
      await page.locator('#physical-param-diameter').fill('2.5 mm');await save();
      assert.match(fs.readFileSync(source,'utf8'),/2.5 mm/);
      assert.equal(await page.locator('.component').count(),0);
    }
    await page.reload();await page.waitForFunction(()=>document.querySelector('#source-state').textContent==='Source ready');
    await page.locator('#tab-features').click();
    if(kind==='macro')await choose('macro: track link');
    if(screenshot)await page.screenshot({path:screenshot,fullPage:true});
    await page.locator('#file-menu > summary').click();
    const undone=page.waitForResponse(r=>r.url().endsWith('/api/operation')&&r.request().postDataJSON()?.action==='undo_source');
    await page.locator('#undo-source').click();
    const response=await undone;assert.equal(response.status(),200,await response.text());
    await page.waitForFunction(()=>document.querySelector('#source-state').textContent==='Source ready');
    assert.deepEqual(fs.readFileSync(source),original,'source undo restores exact bytes');
    assert.deepEqual(errors,[]);console.log('PASS physical editor '+kind);
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
