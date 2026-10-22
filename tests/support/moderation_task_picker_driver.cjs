const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const [script,host]=process.argv.slice(2),scripts=[];
class Element{constructor(tag){this.tagName=tag;this.children=[];this.style={};}appendChild(e){this.children.push(e);e.parent=this;return e;}querySelectorAll(){return this.children.filter(e=>e.tagName==='button');}remove(){this.parent.children=this.parent.children.filter(e=>e!==this);}}
const document={head:{appendChild:s=>scripts.push(s)},body:new Element('body'),readyState:'complete',currentScript:{nonce:'fixture-nonce'},createElement:t=>new Element(t),getElementById:id=>document.body.children.find(e=>e.id===id)};
vm.runInNewContext(fs.readFileSync(script,'utf8'),{window:{},document});
const choices=JSON.parse(fs.readFileSync(script,'utf8').split('const choices=')[1].split(';')[0]);
if(choices.length>1){
 assert.equal(scripts.length,0);const box=document.getElementById('live-archive-task-picker');assert.ok(box);
 const buttons=box.querySelectorAll('button');assert.equal(buttons.length,3);buttons[1].onclick();assert.equal(scripts[0].src,'/__live_archive/moderation/v2/tasks/b/bootstrap.js');assert.equal(document.body.children.length,0);
}else{assert.equal(scripts[0].src,choices[0].prefix+'bootstrap.js');assert.equal(document.body.children.length,0);}
assert.equal(scripts[0].nonce,'fixture-nonce');
