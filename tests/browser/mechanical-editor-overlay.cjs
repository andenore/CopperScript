/* Routed-reference smoke: source bytes stay untouched; placement changes are temporary. */
const assert=require("node:assert/strict");
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||"playwright");
(async()=>{
  const [url,screenshot]=process.argv.slice(2);
  assert(new URL(url).hostname==="127.0.0.1");
  const browser=await chromium.launch({headless:true,executablePath:process.env.COPPER_BROWSER_EXECUTABLE});
  try {
    const page=await browser.newPage({viewport:{width:1600,height:1100}}),errors=[];
    page.on("pageerror",e=>errors.push(e.message));
    await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll(".routed-track").length>0);
    assert.equal(await page.locator(".routed-overlay.stale").count(),0);
    assert.equal(await page.locator(".airwire").count(),0);
    await page.locator("#copper-layer").selectOption("B.Cu");
    assert.equal(await page.locator(".routed-track").count(),0);
    await page.locator("#copper-layer").selectOption("F.Cu");
    assert((await page.locator(".routed-track").count())>0);
    await page.locator("#routed-tracks").uncheck();assert.equal(await page.locator(".routed-track").count(),0);
    await page.locator("#routed-tracks").check();
    await page.locator("#reference").selectOption("R1");
    const x=Number(await page.locator("#x").inputValue());
    await page.locator("#x").fill(String(x+.1));await page.locator("#move").click();
    await page.waitForFunction(()=>!document.querySelector("#apply").disabled);
    assert.equal(await page.locator(".routed-overlay.stale").count(),1);
    assert((await page.locator(".airwire").count())>0);
    await page.locator("#net-labels").check();assert((await page.locator(".net-label").count())>0);
    await page.locator("#net-costs").check();assert.match(await page.locator("#net-cost-summary").innerText(),/MST estimate/);
    await page.locator("#power-filter").selectOption("power");assert.equal(await page.locator(".airwire").count(),0);
    await page.locator("#power-filter").selectOption("all");
    await page.locator("#discard").click();await page.waitForFunction(()=>document.querySelector("#apply").disabled && !document.querySelector("#auto").disabled);
    assert.equal(await page.locator(".routed-overlay.stale").count(),0);
    assert.equal(await page.locator(".airwire").count(),0);
    if(screenshot) await page.screenshot({path:screenshot,fullPage:true});
    assert.deepEqual(errors,[]);
    console.log("PASS: routed layers, filters, stale-preview airwires, net labels/costs and discard restore; zero browser errors");
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
