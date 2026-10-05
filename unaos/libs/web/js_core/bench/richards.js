// Richards: an operating-system task scheduler simulation (after Martin Richards' BCPL benchmark).
var ID_IDLE = 0, ID_WORKER = 1, ID_HANDLER_A = 2, ID_HANDLER_B = 3, ID_DEVICE_A = 4, ID_DEVICE_B = 5;
var KIND_DEVICE = 0, KIND_WORK = 1, DATA_SIZE = 4;
function Packet(link, id, kind) { this.link = link; this.id = id; this.kind = kind; this.a1 = 0; this.a2 = new Array(DATA_SIZE).fill(0); }
Packet.prototype.addTo = function (queue) {
  this.link = null;
  if (queue === null) return this;
  var next = queue; while (next.link !== null) next = next.link;
  next.link = this; return queue;
};
function Tcb(link, id, pri, queue, task) {
  this.link = link; this.id = id; this.pri = pri; this.queue = queue; this.task = task;
  this.state = queue === null ? 2 : 3; // 2 = suspended, 3 = suspended + runnable, 0 = running, 1 = runnable
}
Tcb.prototype.isHeld = function () { return (this.state & 4) !== 0; };
Tcb.prototype.isRunnable = function () { return this.state === 1 || this.state === 0; };
Tcb.prototype.check = function (packet) {
  if (this.queue === null) { this.queue = packet; this.state |= 1; if (this.pri > sched.cur.pri) return this; }
  else this.queue = packet.addTo(this.queue);
  return sched.cur;
};
Tcb.prototype.run = function () {
  var p = null;
  if (this.state === 3) { p = this.queue; this.queue = p.link; this.state = this.queue === null ? 0 : 1; }
  return this.task.run(p);
};
function Scheduler() { this.queueCount = 0; this.holdCount = 0; this.blocks = []; this.list = null; this.cur = null; }
Scheduler.prototype.add = function (id, pri, queue, task) {
  var t = new Tcb(this.list, id, pri, queue, task); this.list = t; this.blocks[id] = t; return t;
};
Scheduler.prototype.schedule = function () {
  this.cur = this.list;
  while (this.cur !== null) {
    if (this.cur.isHeld() || !(this.cur.state === 0 || this.cur.state === 1 || this.cur.state === 3)) this.cur = this.cur.link;
    else this.cur = this.cur.run();
  }
};
Scheduler.prototype.release = function (id) {
  var t = this.blocks[id]; if (t === undefined) return t;
  t.state &= ~4; return t.pri > this.cur.pri ? t : this.cur;
};
Scheduler.prototype.holdCurrent = function () { this.holdCount++; this.cur.state |= 4; return this.cur.link; };
Scheduler.prototype.suspendCurrent = function () { this.cur.state |= 2; return this.cur; };
Scheduler.prototype.queue = function (packet) {
  var t = this.blocks[packet.id]; if (t === undefined) return t;
  this.queueCount++; packet.link = null; packet.id = this.cur.id; return t.check(packet);
};
function IdleTask(v1, count) { this.v1 = v1; this.count = count; }
IdleTask.prototype.run = function () {
  if (--this.count === 0) return sched.holdCurrent();
  if ((this.v1 & 1) === 0) { this.v1 = this.v1 >> 1; return sched.release(ID_DEVICE_A); }
  this.v1 = (this.v1 >> 1) ^ 0xD008; return sched.release(ID_DEVICE_B);
};
function DeviceTask() { this.v1 = null; }
DeviceTask.prototype.run = function (p) {
  if (p === null) { if (this.v1 === null) return sched.suspendCurrent(); var v = this.v1; this.v1 = null; return sched.queue(v); }
  this.v1 = p; return sched.holdCurrent();
};
function WorkerTask(v1, v2) { this.v1 = v1; this.v2 = v2; }
WorkerTask.prototype.run = function (p) {
  if (p === null) return sched.suspendCurrent();
  this.v1 = this.v1 === ID_HANDLER_A ? ID_HANDLER_B : ID_HANDLER_A;
  p.id = this.v1; p.a1 = 0;
  for (var i = 0; i < DATA_SIZE; i++) { this.v2++; if (this.v2 > 26) this.v2 = 1; p.a2[i] = this.v2; }
  return sched.queue(p);
};
function HandlerTask() { this.v1 = null; this.v2 = null; }
HandlerTask.prototype.run = function (p) {
  if (p !== null) { if (p.kind === KIND_WORK) this.v1 = p.addTo(this.v1); else this.v2 = p.addTo(this.v2); }
  if (this.v1 !== null) {
    var count = this.v1.a1, v;
    if (count < DATA_SIZE) {
      if (this.v2 !== null) { v = this.v2; this.v2 = this.v2.link; v.a1 = this.v1.a2[count]; this.v1.a1 = count + 1; return sched.queue(v); }
    } else { v = this.v1; this.v1 = this.v1.link; return sched.queue(v); }
  }
  return sched.suspendCurrent();
};
var sched;
function runRichards() {
  sched = new Scheduler();
  sched.add(ID_IDLE, 0, null, new IdleTask(1, 1000));
  var q = new Packet(null, ID_WORKER, KIND_WORK); q = new Packet(q, ID_WORKER, KIND_WORK);
  sched.add(ID_WORKER, 1000, q, new WorkerTask(ID_HANDLER_A, 0));
  q = new Packet(null, ID_DEVICE_A, KIND_DEVICE); q = new Packet(q, ID_DEVICE_A, KIND_DEVICE); q = new Packet(q, ID_DEVICE_A, KIND_DEVICE);
  sched.add(ID_HANDLER_A, 2000, q, new HandlerTask());
  q = new Packet(null, ID_DEVICE_B, KIND_DEVICE); q = new Packet(q, ID_DEVICE_B, KIND_DEVICE); q = new Packet(q, ID_DEVICE_B, KIND_DEVICE);
  sched.add(ID_HANDLER_B, 3000, q, new HandlerTask());
  sched.add(ID_DEVICE_A, 4000, null, new DeviceTask());
  sched.add(ID_DEVICE_B, 5000, null, new DeviceTask());
  sched.schedule();
  return sched.queueCount * 10000 + sched.holdCount;
}
var r = 0;
for (var i = 0; i < 1500; i++) r = runRichards();
print("richards " + r);
