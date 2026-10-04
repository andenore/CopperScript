/* Read-only real-footprint smoke for examples/mechanical_profile_project. */
const assert=require("node:assert/strict");
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||"playwright");
(async()=>{
  const url=process.argv[2];assert(url && new URL(url).hostname==="127.0.0.1");
  const browser=await chromium.launch({headless:true,
    ...(process.env.COPPER_BROWSER_EXECUTABLE ? {executablePath:process.env.COPPER_BROWSER_EXECUTABLE} : {})});
  try {
    const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[];
    page.on("pageerror",e=>errors.push(e.message));
    await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll(".component").length===1);
    await page.locator("#reference").selectOption("J_DEBUG");
    assert.match(await page.locator("#details").innerText(),/Imported profile role: host\/debug \(read-only\)/);
    assert(await page.locator("#move").isDisabled());
    assert.equal(await page.locator(".hole").count(),3);
    const titles=await page.locator(".hole title").allTextContents();
    assert.equal(titles.filter(t=>t.includes("Imported (read-only)")).length,2);
    assert.equal(titles.filter(t=>t.includes("Project-owned")).length,1);
    assert.match(await page.locator(".outline title").textContent(),/Imported \(read-only\)/);
    assert.equal(await page.locator(".keepout").count(),1);
    assert.equal(await page.locator(".copper-keepout").count(),1);
    assert.deepEqual(errors,[]);
    if(process.argv[3])await page.screenshot({path:process.argv[3],fullPage:true});
    console.log("PASS: real connector, imported anchored pose lock, profile/local feature ownership, holes/keepout and zero page errors; no mutations");
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
