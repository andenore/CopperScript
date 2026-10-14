/* Physical artwork preview and filters; source bytes remain untouched. */
const assert=require("node:assert/strict"),fs=require("node:fs");
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||"playwright");
const {tab}=require("./editor-tools.cjs");
(async()=>{
  const [url,source,screenshot]=process.argv.slice(2),original=fs.readFileSync(source);
  const browser=await chromium.launch({headless:true,executablePath:process.env.COPPER_BROWSER_EXECUTABLE});
  try {
    const page=await browser.newPage({viewport:{width:1600,height:1000}}),errors=[];
    page.on("pageerror",e=>errors.push(e.message));
    await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll(".component").length===2);
    const artwork=page.locator(".board-artwork"),features=artwork.locator("[data-artwork-id]");
    assert.equal(await features.count(),14,"Default branding and all eight source features render");
    assert.equal(await page.locator('[data-artwork-id="__copperscript:name"] .artwork-text').textContent(),"CopperScript");
    assert.match(await page.locator('[data-artwork-id="__copperscript:version"] .artwork-text').textContent(),/^v/);
    assert.equal(await page.locator('[data-artwork-id="FRONT_LABEL"] tspan').count(),2);
    assert.match(await page.locator('[data-artwork-id="BACK_LABEL"] text').getAttribute("transform"),/rotate\(90\) scale\(-1 1\)/);
    assert.equal(await page.locator('[data-artwork-id="ARC"] path').count(),1);
    assert.equal(await page.locator('[data-artwork-id="CIRCLE"] circle').count(),1);
    assert.equal(await page.locator('[data-artwork-id="RECT"] polygon').count(),1);
    assert.equal(await page.locator('[data-artwork-id="TRIANGLE"] polygon').getAttribute("fill"),"currentColor");
    assert.equal(await page.locator(".footprint-artwork line").count()>0,true);
    assert.equal(await artwork.evaluateAll(ns=>ns.every(n=>getComputedStyle(n).pointerEvents==="visiblepainted")),true);
    assert.equal(await page.locator('.footprint-artwork').evaluateAll(ns=>ns.every(n=>getComputedStyle(n).pointerEvents==="none")),true);
    await page.locator('[data-artwork-id="FRONT_LABEL"]').dispatchEvent('click');
    assert.equal(await page.locator('#tab-features').getAttribute('aria-selected'),'true');
    assert.equal(await page.locator('#physical-name').inputValue(),'FRONT_LABEL');
    assert.equal(await page.locator('#physical-param-text').inputValue(),'"Front\\nRev A"');
    await tab(page,"view");
    await page.locator("#side").selectOption("back");
    assert.equal(await features.count(),3,"Only rear label/circle/copper remain");
    assert.equal(await page.locator(".footprint-artwork.front").count(),0);
    await page.locator("#silkscreen").uncheck();
    assert.equal(await features.count(),1);assert.equal(await page.locator(".footprint-artwork").count(),0);
    assert.equal(await page.locator('[data-artwork-id="COPPER"]').count(),1);
    await page.locator("#copper-artwork").uncheck();assert.equal(await features.count(),0);
    await page.locator("#silkscreen").check();await page.locator("#side").selectOption("front");
    assert.equal(await features.count(),11);
    await page.locator("#side").selectOption("both");await page.locator("#copper-artwork").check();
    await page.locator("#net").selectOption("SIGNAL");
    assert.equal(await features.count(),14,"Net filtering cannot hide decorative artwork");
    if(screenshot)await page.screenshot({path:screenshot,fullPage:true});
    assert.deepEqual(errors,[]);assert.deepEqual(fs.readFileSync(source),original);
    console.log("PASS: board/footprint silkscreen, Cu branding/version, shapes, mirrored multiline text, independent side/material filters; source unchanged");
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
