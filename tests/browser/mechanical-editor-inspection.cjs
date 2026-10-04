/* Read-only browser inspection for the real circular LED-ring project. */
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
    await page.goto(url);await page.waitForFunction(()=>document.querySelectorAll(".component").length>0);
    const scene=await page.evaluate(async()=>{
      const token=new URLSearchParams(location.hash.slice(1)).get("token");
      return (await fetch("/api/scene",{headers:{"X-Copper-Token":token}})).json();
    });
    assert(scene.outline.circle,"Expected a circular board");
    assert.equal(await page.locator("circle.outline").count(),1);
    assert.equal(await page.locator(".component").count(),scene.components.length);
    const front=scene.components.filter(c=>c.side==="front").length;
    const back=scene.components.filter(c=>c.side==="back").length;
    assert(front>0 && back>0,"Must inspect both LEDs/MCU and rear battery");
    await page.locator("#side").selectOption("back");assert.equal(await page.locator(".component").count(),back);
    await page.locator("#side").selectOption("front");assert.equal(await page.locator(".component").count(),front);
    await page.locator("#side").selectOption("both");
    const b=await page.locator("#board").boundingBox();
    const before=await page.locator("#board").getAttribute("viewBox");
    await page.mouse.move(b.x+b.width/2,b.y+b.height/2);await page.mouse.wheel(0,-100);
    await page.waitForFunction(v=>document.querySelector("#board").getAttribute("viewBox")!==v,before);
    await page.getByRole("button",{name:"Fit board",exact:true}).click();
    assert.equal(await page.locator("#board").getAttribute("viewBox"),before);
    if(process.argv[3])await page.screenshot({path:process.argv[3],fullPage:true});
    assert.deepEqual(errors,[]);
    console.log(`PASS: real LED ring, circle, ${front} front/${back} rear components, wheel/fit, zero page errors; no mutations`);
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
