/* Optional headless browser smoke. Run against a freshly started
 * mechanical_editor_demo.copper session; this test changes temporary poses/locks.
 * npm/Playwright are test-only dependencies, not CopperScript runtime deps.
 * COPPER_PLAYWRIGHT_MODULE / COPPER_BROWSER_EXECUTABLE allow explicit installed tools.
 */
const assert = require("node:assert/strict");
const {chromium} = require(process.env.COPPER_PLAYWRIGHT_MODULE || "playwright");

(async () => {
  const url = process.argv[2];
  assert(url && new URL(url).hostname === "127.0.0.1", "Supply the editor's loopback launch URL");
  const browser = await chromium.launch({headless:true,
    ...(process.env.COPPER_BROWSER_EXECUTABLE ? {executablePath:process.env.COPPER_BROWSER_EXECUTABLE} : {})});
  try {
    const page = await browser.newPage({viewport:{width:1440,height:1000}});
    const errors = [];
    page.on("pageerror",error=>errors.push(error.message));
    await page.goto(url);
    await page.waitForFunction(()=>document.querySelectorAll(".component").length===4);
    const initialRevision=await page.evaluate(async()=>{
      const token=new URLSearchParams(location.hash.slice(1)).get("token");
      return (await (await fetch("/api/scene",{headers:{"X-Copper-Token":token}})).json()).revision;
    });
    assert.equal(initialRevision,0,"Run the mutating smoke against a freshly started demo session");
    assert.match(await page.locator("#notice").innerText(),/Source edits require/);
    assert(await page.locator(".airwire").count()>0);
    await page.locator("#airwires").uncheck();
    assert.equal(await page.locator(".airwire").count(),0);
    await page.locator("#airwires").check();
    await page.locator("#net").selectOption("SIGNAL");
    assert.equal(await page.locator(".airwire").count(),3);
    await page.getByRole("button",{name:"Preview rough auto-placement"}).click();
    await page.waitForFunction(()=>!document.querySelector("#apply").disabled);
    await page.getByRole("button",{name:"Apply preview",exact:true}).click();
    await page.waitForFunction(()=>!document.querySelector("#undo").disabled);
    await page.locator("#reference").selectOption("R1");
    await page.locator("#x").fill("20"); await page.locator("#y").fill("12");
    await page.locator("#rotation").fill("45");
    await page.getByRole("button",{name:"Preview pose",exact:true}).click();
    await page.waitForFunction(()=>!document.querySelector("#apply").disabled);
    assert.equal(await page.locator("#rotation").inputValue(),"45");
    await page.getByRole("button",{name:"Apply preview",exact:true}).click();
    await page.waitForFunction(()=>!document.querySelector("#move").disabled);
    await page.getByRole("button",{name:"Lock temporary pose",exact:true}).click();
    await page.getByRole("button",{name:"Unlock temporary pose",exact:true}).waitFor();
    await page.getByRole("button",{name:"Undo",exact:true}).click();
    await page.getByRole("button",{name:"Lock temporary pose",exact:true}).waitFor();
    await page.getByRole("button",{name:"Redo",exact:true}).click();
    await page.getByRole("button",{name:"Unlock temporary pose",exact:true}).waitFor();
    await page.locator("#side").selectOption("back");
    assert.equal(await page.locator(".component").count(),0);
    await page.locator("#side").selectOption("both");
    await page.locator("#selected-only").check();
    assert.equal(await page.locator(".component").count(),4);
    await page.getByRole("button",{name:"Unlock temporary pose",exact:true}).click();
    await page.getByRole("button",{name:"Lock temporary pose",exact:true}).waitFor();
    const body = await page.locator('[data-reference="R1"] .body').boundingBox();
    const wiresBefore = await page.locator(".airwire").evaluateAll(nodes=>nodes.map(n=>n.outerHTML));
    await page.mouse.move(body.x+body.width/2,body.y+body.height/2); await page.mouse.down();
    await page.mouse.move(body.x+body.width/2+35,body.y+body.height/2+10,{steps:5});
    const wiresDuring = await page.locator(".airwire").evaluateAll(nodes=>nodes.map(n=>n.outerHTML));
    assert.notDeepEqual(wiresDuring,wiresBefore,"Ratsnest must follow the component during drag");
    await page.mouse.up();
    await page.waitForFunction(()=>!document.querySelector("#move").disabled);
    assert(await page.locator("#apply").isDisabled(),"Default drag must accept its checked pose");
    const firstX=Number(await page.locator("#x").inputValue());
    async function dragR1(dx,dy) {
      const box=await page.locator('[data-reference="R1"] .body').boundingBox();
      await page.mouse.move(box.x+box.width/2,box.y+box.height/2);await page.mouse.down();
      await page.mouse.move(box.x+box.width/2+dx,box.y+box.height/2+dy,{steps:5});await page.mouse.up();
    }
    await dragR1(35,0);
    await page.waitForFunction(()=>!document.querySelector("#move").disabled);
    assert(Number(await page.locator("#x").inputValue())>firstX,"A second drag must work without Apply");
    assert(await page.locator("#apply").isDisabled());
    const legalX=await page.locator("#x").inputValue();
    await page.locator("#x").fill("10000");await page.locator("#move").click();
    await page.waitForFunction(()=>document.querySelector("#status").classList.contains("error") && !document.querySelector("#move").disabled);
    assert.equal(await page.locator("#x").inputValue(),legalX,"Rejected move must preserve the accepted pose and permit further editing");
    await page.locator("#preview-drags").check();
    await dragR1(15,0);
    await page.waitForFunction(()=>!document.querySelector("#apply").disabled);
    assert(await page.locator("#pending").isVisible());
    await page.getByRole("button",{name:"Discard preview",exact:true}).click();
    await page.waitForFunction(()=>document.querySelector("#apply").disabled);
    await page.locator("#preview-drags").uncheck();
    const viewport=await page.locator("#board").boundingBox();
    const cursor={x:viewport.x+viewport.width*.4,y:viewport.y+viewport.height*.6};
    async function worldPoint() {
      return page.locator("#board").evaluate((svg,c)=>{
        const p=new DOMPoint(c.x,c.y).matrixTransform(svg.getScreenCTM().inverse());
        return [p.x,p.y];
      },cursor);
    }
    const anchorBefore=await worldPoint();
    const viewBefore=await page.locator("#board").getAttribute("viewBox");
    await page.mouse.move(cursor.x,cursor.y);await page.mouse.wheel(0,-150);
    await page.waitForFunction(v=>document.querySelector("#board").getAttribute("viewBox")!==v,viewBefore);
    const anchorAfter=await worldPoint();
    assert(Math.hypot(anchorBefore[0]-anchorAfter[0],anchorBefore[1]-anchorAfter[1])<1e-5,"Wheel zoom must keep the world point under the cursor");
    const poseBefore=await page.locator("#x").inputValue();
    const zoomed=(await page.locator("#board").getAttribute("viewBox")).split(" ").map(Number);
    // Pan starting on a footprint, proving right drag does not initiate a pose move.
    const panBody=await page.locator('[data-reference="R1"] .body').boundingBox();
    await page.mouse.move(panBody.x+panBody.width/2,panBody.y+panBody.height/2);await page.mouse.down({button:"right"});
    await page.mouse.move(panBody.x+panBody.width/2+80,panBody.y+panBody.height/2+40,{steps:8});
    await page.mouse.up({button:"right"});
    const panned=(await page.locator("#board").getAttribute("viewBox")).split(" ").map(Number);
    assert(panned[0]<zoomed[0] && panned[1]<zoomed[1]);
    assert.deepEqual(panned.slice(2),zoomed.slice(2));
    assert.equal(await page.locator("#x").inputValue(),poseBefore);
    assert(await page.locator("#apply").isDisabled());
    // A second client advances the revision. The first must recover after 409.
    await page.evaluate(async()=>{
      const token=new URLSearchParams(location.hash.slice(1)).get("token");
      const headers={"X-Copper-Token":token,"Content-Type":"application/json"};
      const s=await (await fetch("/api/scene",{headers})).json();
      const r=await fetch("/api/operation",{method:"POST",headers,body:JSON.stringify({action:"lock",revision:s.revision,reference:"R2",locked:true})});
      if(!r.ok)throw new Error("Second-client setup failed");
    });
    await page.locator("#lock").click();
    await page.waitForFunction(()=>document.querySelector("#status").textContent.includes("Scene reloaded"));
    await page.locator("#lock").click();
    await page.getByRole("button",{name:"Unlock temporary pose",exact:true}).waitFor();
    if(process.argv[3]) await page.screenshot({path:process.argv[3],fullPage:true});
    assert.deepEqual(errors,[]);
    console.log("PASS: repeated accepted drags, explicit previews, cursor-anchored wheel zoom, right-drag pan, stale revision recovery, real footprints/ratsnest, auto-place, 45-degree pose, locks/undo/redo; no page errors");
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
