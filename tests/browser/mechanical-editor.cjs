/* Optional headless browser smoke. Run against mechanical_editor_demo.copper.
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
    assert.match(await page.locator("#notice").innerText(),/read-only/);
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
    await page.waitForFunction(()=>!document.querySelector("#apply").disabled);
    await page.getByRole("button",{name:"Discard preview",exact:true}).click();
    await page.waitForFunction(()=>document.querySelector("#apply").disabled);
    if(process.argv[3]) await page.screenshot({path:process.argv[3],fullPage:true});
    assert.deepEqual(errors,[]);
    console.log("PASS: real-footprint view, ratsnest toggles/net/side filters and live drag, auto-place preview/apply, 45-degree pose, temporary lock, undo/redo; no page errors");
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
