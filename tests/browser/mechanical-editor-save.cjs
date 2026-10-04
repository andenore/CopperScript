/* Mutating smoke for a disposable copy of mechanical_editor_demo.copper. */
const assert=require("node:assert/strict");
const fs=require("node:fs");
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||"playwright");
(async()=>{
  const [url,source,screenshot]=process.argv.slice(2);
  assert(new URL(url).hostname==="127.0.0.1");
  assert(source && source.includes("build"),"Use a disposable build/ source copy");
  const original=fs.readFileSync(source);
  const browser=await chromium.launch({headless:true,executablePath:process.env.COPPER_BROWSER_EXECUTABLE});
  try {
    const page=await browser.newPage({viewport:{width:1600,height:1100}}),errors=[];
    page.on("pageerror",e=>errors.push(e.message));
    await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll(".component").length===4);
    await page.locator("#auto").click();await page.waitForFunction(()=>!document.querySelector("#apply").disabled);
    await page.locator("#apply").click();await page.waitForFunction(()=>document.querySelector("#apply").disabled);
    await page.locator("#reference").selectOption("R_ANCHOR");
    await page.locator("#edit-locks").check();
    await page.locator("#x").fill("8");
    await page.locator("#pose-side").selectOption("back");
    await page.locator("#prepare-lock").click();
    await page.waitForFunction(()=>!document.querySelector("#save-source").disabled);
    assert.match(await page.locator("#source-diff").innerText(),/x = 8mm/);
    assert.deepEqual(fs.readFileSync(source),original,"Review must not write source");
    await page.locator("#save-source").click();
    await page.waitForFunction(()=>!document.querySelector("#undo-source").disabled);
    assert.match(fs.readFileSync(source,"utf8"),/x = 8mm; y = 8mm/);
    await page.locator("#undo-source").click();await page.waitForFunction(()=>!document.querySelector("#redo-source").disabled);
    assert.deepEqual(fs.readFileSync(source),original);
    await page.locator("#redo-source").click();await page.waitForFunction(()=>!document.querySelector("#undo-source").disabled);
    await page.locator("#feature").selectOption("2"); // H1 after outline/cutout.
    await page.locator("#feature-param-diameter").fill("2.4mm");
    await page.locator("#prepare-feature").click();await page.waitForFunction(()=>!document.querySelector("#save-source").disabled);
    assert.match(await page.locator("#source-diff").innerText(),/2.4mm/);
    await page.locator("#save-source").click();await page.waitForFunction(()=>document.querySelector("#source-review").hidden && !document.querySelector("#auto").disabled);
    assert.match(fs.readFileSync(source,"utf8"),/diameter = 2.4mm/);
    await page.locator("#measure").check();
    const view=await page.locator("#board").boundingBox();
    await page.mouse.click(view.x+view.width*.4,view.y+view.height*.4);
    await page.mouse.click(view.x+view.width*.6,view.y+view.height*.4);
    assert.equal(await page.locator(".measure-line").count(),1);
    assert.match(await page.locator("#measurement").innerText(),/mm; ΔX/);
    if(screenshot) await page.screenshot({path:screenshot,fullPage:true});
    assert.deepEqual(errors,[]);
    // Undo both edits; every source byte must return to the original.
    await page.locator("#undo-source").click();await page.waitForFunction(()=>!document.querySelector("#redo-source").disabled);
    await page.locator("#undo-source").click();await page.waitForFunction(()=>document.querySelector("#undo-source").disabled && !document.querySelector("#auto").disabled);
    assert.deepEqual(fs.readFileSync(source),original);
    console.log("PASS: reviewed pose/mechanics save, persisted undo/redo, measurement; zero browser errors");
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
