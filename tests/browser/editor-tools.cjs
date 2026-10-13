exports.tab=async(page,name)=>page.locator("#tab-"+name).click();
exports.expand=async(page,id)=>{
  const details=page.locator("#"+id);
  if(!await details.evaluate(node=>node.open))await details.locator("summary").first().click();
};
exports.sourceAction=async(page,id)=>{
  await exports.expand(page,"file-menu");
  await page.locator("#"+id).click();
};
