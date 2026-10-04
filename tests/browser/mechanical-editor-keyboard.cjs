/* Accessibility/keyboard regression against a disposable editor source. */
const assert=require("node:assert/strict"),fs=require("node:fs");
const {chromium}=require(process.env.COPPER_PLAYWRIGHT_MODULE||"playwright");
(async()=>{
  const [url,source,screenshot]=process.argv.slice(2);assert(source.includes("build"));
  const original=fs.readFileSync(source);
  const browser=await chromium.launch({headless:true,executablePath:process.env.COPPER_BROWSER_EXECUTABLE});
  try {
    const page=await browser.newPage({viewport:{width:1600,height:1100}}),errors=[];page.on("pageerror",e=>errors.push(e.message));
    await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll(".component").length===4);
    await page.locator("#auto").click();await page.waitForFunction(()=>!document.querySelector("#apply").disabled);
    await page.locator("#apply").click();await page.waitForFunction(()=>!document.querySelector("#auto").disabled);
    await page.locator("#reference").selectOption("R1");
    await page.locator("#x").fill("20");await page.locator("#y").fill("12");await page.locator("#rotation").fill("0");
    await page.locator("#move").click();await page.waitForFunction(()=>!document.querySelector("#apply").disabled);
    await page.locator("#apply").click();await page.waitForFunction(()=>!document.querySelector("#auto").disabled);
    const board=page.locator("#board");await board.focus();await page.keyboard.press("ArrowRight");
    await page.waitForFunction(()=>document.querySelector("#x").value==="20.1" && !document.querySelector("#auto").disabled);
    await page.keyboard.press("Shift+ArrowDown");
    await page.waitForFunction(()=>document.querySelector("#y").value==="13" && !document.querySelector("#auto").disabled);
    await page.keyboard.press("r");
    await page.waitForFunction(()=>document.querySelector("#rotation").value==="45" && !document.querySelector("#auto").disabled);
    const component=page.getByRole("button",{name:/^R1, /});assert.equal(await component.count(),1);
    await component.focus();await page.keyboard.press("Enter");assert.equal(await component.getAttribute("aria-pressed"),"true");
    await board.focus();const fit=await board.getAttribute("viewBox");await page.keyboard.press("+");assert.notEqual(await board.getAttribute("viewBox"),fit);
    await page.keyboard.press("f");assert.equal(await board.getAttribute("viewBox"),fit);
    await page.keyboard.press("Escape");assert.equal(await page.locator("#measurement").innerText(),"No measurement");
    if(screenshot)await page.screenshot({path:screenshot,fullPage:true});
    assert.deepEqual(errors,[]);assert.deepEqual(fs.readFileSync(source),original);
    console.log("PASS: accessible footprint roles, arrows/Shift snaps, allowed 45-degree rotation, Enter/focus, keyboard zoom/fit/Escape; source unchanged");
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
