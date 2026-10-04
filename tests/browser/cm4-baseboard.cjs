/* Read-only validation and screenshot of the real CM4 carrier example. */
const assert=require("node:assert/strict");
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||"playwright");
(async()=>{
  const url=process.argv[2];
  assert(url && new URL(url).hostname==="127.0.0.1");
  const browser=await chromium.launch({headless:true,
    ...(process.env.COPPER_BROWSER_EXECUTABLE ? {executablePath:process.env.COPPER_BROWSER_EXECUTABLE} : {})});
  try {
    const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[];
    page.on("pageerror",e=>errors.push(e.message));
    await page.goto(url);
    await page.waitForFunction(()=>document.querySelectorAll(".component").length===9);
    const scene=await page.evaluate(async()=>{
      const token=new URLSearchParams(location.hash.slice(1)).get("token");
      return (await fetch("/api/scene",{headers:{"X-Copper-Token":token}})).json();
    });
    assert.equal(scene.board,"CM4Baseboard");
    assert.equal(scene.holes.length,4);
    assert.equal(scene.copper_keepouts.length,1);
    assert.equal(scene.keepouts.length,5);
    assert(scene.placement_legal);
    const sockets=scene.components.filter(c=>c.reference.startsWith("J_CM4_"));
    assert.equal(sockets.length,2);
    assert(sockets.every(c=>c.pads.length===100 && c.source_position_locked && c.profile_role));
    assert(await page.locator(".airwire").count()>0);
    await page.locator("#airwires").uncheck();
    assert.equal(await page.locator(".airwire").count(),0);
    if(process.argv[3])await page.screenshot({path:process.argv[3],fullPage:true});
    assert.deepEqual(errors,[]);
    console.log("PASS: CM4 carrier, two locked 100-pad sockets, four holes, body/antenna keepouts, legal placement, no mutations");
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
