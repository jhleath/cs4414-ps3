//
// zhtta.rs
//
// Starting code for PS3
// Running on Rust 0.9
//
// Note that this code has serious security risks!  You should not run it 
// on any system with access to sensitive files.
// 
// University of Virginia - cs4414 Spring 2014
// Weilin Xu and David Evans
// Version 0.5

// To see debug! outputs set the RUST_LOG environment variable, e.g.: export RUST_LOG="zhtta=debug"

#[feature(globs)];
extern mod extra;

use std::io::*;
use std::io::net::ip::{SocketAddr};
use std::{os, str, libc, from_str, path, run};
use std::path::Path;
use std::hashmap::HashMap;
use std::io::buffered::BufferedReader;
use std::io::pipe::PipeStream;

use extra::getopts;
use extra::arc::{MutexArc,RWArc};
use extra::priority_queue::PriorityQueue;

static SERVER_NAME : &'static str = "Zhtta Version 0.5";

static NUM_PROCESS : uint = 4;

static IP : &'static str = "127.0.0.1";
static PORT : uint = 4414;
static WWW_DIR : &'static str = "./www";

static HTTP_OK : &'static str = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n";
static HTTP_BAD : &'static str = "HTTP/1.1 404 Not Found\r\n\r\n";

static COUNTER_STYLE : &'static str = "<doctype !html><html><head><title>Hello, Rust!</title>
             <style>body { background-color: #884414; color: #FFEEAA}
                    h1 { font-size:2cm; text-align: center; color: black; text-shadow: 0 0 4mm red }
                    h2 { font-size:2cm; text-align: center; color: black; text-shadow: 0 0 4mm green }
             </style></head>
             <body>";

//static mut visitor_count : uint = 0;

struct HTTP_Request {
    // Use peer_name as the key to access TcpStream in hashmap. 

    // (Due to a bug in extra::arc in Rust 0.9, it is very inconvenient to use TcpStream without the "Freeze" bound.
    //  See issue: https://github.com/mozilla/rust/issues/12139)
    peer_name: ~str,
    path: ~Path,
    priority: uint,
}

impl Ord for HTTP_Request {
    fn lt(&self, other : &HTTP_Request) -> bool {
        return self.priority < other.priority
    }
}

struct WebServer {
    ip: ~str,
    port: uint,
    www_dir_path: ~Path,
    
    request_queue_arc: MutexArc<PriorityQueue<~HTTP_Request>>,
    stream_map_arc: MutexArc<HashMap<~str, Option<std::io::net::tcp::TcpStream>>>,
    visitor_count_arc: RWArc<uint>,
    cache_arc: RWArc<HashMap<Path, ~[u8]>>,
    
    notify_port: Port<()>,
    shared_notify_chan: SharedChan<()>,
}

impl WebServer {
    fn new(ip: &str, port: uint, www_dir: &str) -> WebServer {
        let (notify_port, shared_notify_chan) = SharedChan::new();
        let www_dir_path = ~Path::new(www_dir);
        os::change_dir(www_dir_path.clone());

        WebServer {
            ip: ip.to_owned(),
            port: port,
            www_dir_path: www_dir_path,
                        
            request_queue_arc: MutexArc::new(PriorityQueue::new()),
            stream_map_arc: MutexArc::new(HashMap::new()),
            visitor_count_arc: RWArc::new(0),
            cache_arc: RWArc::new(HashMap::new()),
            
            notify_port: notify_port,
            shared_notify_chan: shared_notify_chan,        
        }
    }
    
    fn run(&mut self) {
        self.listen();
        self.dequeue_static_file_request();
    }
    
    fn listen(&mut self) {
        let addr = from_str::<SocketAddr>(format!("{:s}:{:u}", self.ip, self.port)).expect("Address error.");

        let www_dir_path_str = self.www_dir_path.as_str().expect("invalid www path?").to_owned();

        let request_queue_arc = self.request_queue_arc.clone();
        let shared_notify_chan = self.shared_notify_chan.clone();
        let stream_map_arc = self.stream_map_arc.clone();
        let visitor_count_arc = self.visitor_count_arc.clone();

        spawn(proc() {;
            let mut acceptor = net::tcp::TcpListener::bind(addr).listen();
            println!("{:s} listening on {:s} (serving from: {:s}).", 
                     SERVER_NAME, addr.to_str(), www_dir_path_str);
            
            for stream in acceptor.incoming() {
                let (queue_port, queue_chan) = Chan::new();
                queue_chan.send(request_queue_arc.clone());
                
                let (count_port, count_chan) = Chan::new();
                count_chan.send(visitor_count_arc.clone());
                
                let notify_chan = shared_notify_chan.clone();
                let stream_map_arc = stream_map_arc.clone();
                
                // Spawn a task to handle the connection.
                spawn(proc() {
                    // DONE: Fix unsafe counter
                    let visitor_count_arc = count_port.recv();
                    visitor_count_arc.write(|visitor_count| {
                        *visitor_count += 1;
                    });
                    
                    let request_queue_arc = queue_port.recv();
                  
                    let mut stream = stream;
                    
                    let peer_name = WebServer::get_peer_name(&mut stream);
                    
                    let mut buf = [0, ..500];
                    stream.read(buf);
                    let request_str = str::from_utf8(buf);
                    debug!("Request:\n{:s}", request_str);
                    
                    let req_group : ~[&str]= request_str.splitn(' ', 3).collect();
                    if req_group.len() > 2 {
                        let path_str = "." + req_group[1].to_owned();
                        
                        let mut path_obj = ~os::getcwd();
                        path_obj.push(path_str.clone());
                        
                        let ext_str = match path_obj.extension_str() {
                            Some(e) => e,
                            None => "",
                        };
                        
                        debug!("Requested path: [{:s}]", path_obj.as_str().expect("error"));
                        debug!("Requested path: [{:s}]", path_str);
                             
                        if path_str == ~"./" {
                            debug!("===== Counter Page request =====");
                            WebServer::respond_with_counter_page(stream, visitor_count_arc);
                            debug!("=====Terminated connection from [{:s}].=====", peer_name);
                        } else if !path_obj.exists() || path_obj.is_dir() {
                            debug!("===== Error page request =====");
                            WebServer::respond_with_error_page(stream, path_obj);
                            debug!("=====Terminated connection from [{:s}].=====", peer_name);
                        } else if ext_str == "shtml" { // Dynamic web pages.
                            debug!("===== Dynamic Page request =====");
                            WebServer::respond_with_dynamic_page(stream, path_obj);
                            debug!("=====Terminated connection from [{:s}].=====", peer_name);
                        } else { 
                            debug!("===== Static Page request =====");
                            WebServer::enqueue_static_file_request(stream, path_obj, stream_map_arc, request_queue_arc, notify_chan);
                        }
                    }
                });
            }
        });
    }

    fn respond_with_error_page(stream: Option<std::io::net::tcp::TcpStream>, path: &Path) {
        let mut stream = stream;
        let msg: ~str = format!("Cannot open: {:s}", path.as_str().expect("invalid path").to_owned());

        stream.write(HTTP_BAD.as_bytes());
        stream.write(msg.as_bytes());
    }

    // DONE: Safe visitor counter.
    fn respond_with_counter_page(stream: Option<std::io::net::tcp::TcpStream>, visitor_count_arc: RWArc<uint>) {
        let mut stream = stream;
        let mut response_visitor_count = 0;
        visitor_count_arc.read(|visitor_count| {
            response_visitor_count = *visitor_count;
        });
        let response: ~str = 
            format!("{:s}{:s}<h1>Greetings, Krusty!</h1>
                     <h2>Visitor count: {:u}</h2></body></html>\r\n", 
                    HTTP_OK, COUNTER_STYLE, 
                    response_visitor_count );
        debug!("Responding to counter request");
        stream.write(response.as_bytes());
    }
    
    // TODO: Streaming file.
    // TODO: Application-layer file caching.
    fn respond_with_static_file(stream: Option<std::io::net::tcp::TcpStream>, path: &Path, cache_arc: RWArc<HashMap<Path, ~[u8]>>) {
        let mut stream = stream;

        let mut file_content: ~[u8] = ~[];
        let mut in_cache = false;

        cache_arc.read(|cache_map| {

            if cache_map.contains_key(path) {
                in_cache = true;
                file_content = cache_map.get(path).to_owned();
                // file_content = ~[072, 101, 108, 108, 111, 044, 032, 087, 111, 114, 108, 100];
            }
            else {
                in_cache = false;
            }
        });
        if in_cache {
            stream.write(HTTP_OK.as_bytes());
            stream.write(file_content);
        }
        else {
            cache_arc.write(|cache_map| {
                let new_path = path.clone();
                let mut file_reader = File::open(path).expect("Invalid file!");

                stream.write(HTTP_OK.as_bytes());

                let mut total_file : ~[u8] = ~[];

                debug!("Writing Bytes");
                loop {
                    let mut bytes = ~[0, .. 1024];
                    file_reader.read(bytes);

                    stream.write(bytes);

                    total_file.push_all_move(bytes);

                    if file_reader.eof() { break; }
                }

                cache_map.insert(new_path, total_file.to_owned());
            });
        }
        debug!("Done");
    }
    
    // DONE: Server-side gashing.
    fn respond_with_dynamic_page(stream: Option<std::io::net::tcp::TcpStream>, path: &Path) {
        // for now, just serve as static file
        // WebServer::respond_with_static_file(stream, path);
        let mut stream = stream;
        let mut file_reader = File::open(path);
        stream.write(HTTP_OK.as_bytes());
        match file_reader {
            Some(file) => {
                let mut reader = BufferedReader::new(file);
                let mut file_str = reader.read_to_str();
                let comment_st = "<!--";
                let comment_en = "-->";
                let command_str = "#exec cmd=";
                loop {
                match file_str.find_str(comment_st) {
                    Some(ind) => {
                        stream.write_str(file_str.slice_to(ind));
                        let ind_end = file_str.find_str(comment_en).unwrap();
                        let tmp_str = file_str.slice(ind, ind_end+comment_en.len()).to_owned();
                        match tmp_str.find_str(command_str) {
                            Some(command_ind_st) => {
                                let command_tail = tmp_str.slice_from(command_ind_st+command_str.len()+1);
                                let command_ind_en = command_tail.find_str("\"").unwrap();
                                let command = command_tail.slice_to(command_ind_en);
                        
                                // execute command with gash
                                // stream.write_str(command);
                                stream.write_str(run_cmdline(command));
                            }
                            None => ()
                        }
                        
                        stream.write_str(file_str.slice(ind, ind_end+comment_en.len()));
                        
                        file_str = file_str.slice_from(ind_end+comment_en.len()).to_owned();
                    }
                    None => { stream.write_str(file_str); break; }
                }
                }
            }
            None => {
                debug!("Error opening file!");
            }
        }
    }
    
    // TODO: Smarter Scheduling.
    fn enqueue_static_file_request(stream: Option<std::io::net::tcp::TcpStream>, path_obj: &Path, stream_map_arc: MutexArc<HashMap<~str, Option<std::io::net::tcp::TcpStream>>>, req_queue_arc: MutexArc<PriorityQueue<~HTTP_Request>>, notify_chan: SharedChan<()>) {
        // Save stream in hashmap for later response.
        let mut stream = stream;
        let peer_name = WebServer::get_peer_name(&mut stream);
        let (stream_port, stream_chan) = Chan::new();
        stream_chan.send(stream);
        unsafe {
            // Use an unsafe method, because TcpStream in Rust 0.9 doesn't have "Freeze" bound.
            stream_map_arc.unsafe_access(|local_stream_map| {
                let stream = stream_port.recv();
                local_stream_map.swap(peer_name.clone(), stream);
            });
        }
        
        // Enqueue the HTTP request.
        let mut req = HTTP_Request { peer_name: peer_name.clone(), path: ~path_obj.clone(), priority: 0 };

        match req.path.stat().size.to_uint() {
            Some(file_size) => req.priority -= file_size,
            None => {}
        }

        // Check to See if From CVille
        let pn = peer_name;
        match pn.find_str("128.143") {
            Some(ind) => { 
                if ind == 0 {
                    req.priority += 1000;
                }
            }
            None => {}
        }
        match pn.find_str("137.54") {
            Some(ind) => { 
                if ind == 0 {
                    req.priority += 1000;
                }
            }
            None => {}
        }

        let (req_port, req_chan) = Chan::new();
        req_chan.send(req);

        debug!("Waiting for queue mutex lock.");
        req_queue_arc.access(|local_req_queue| {
            debug!("Got queue mutex lock.");
            let req: HTTP_Request = req_port.recv();
            local_req_queue.push(~req);
            debug!("A new request enqueued, now the length of queue is {:u}.", local_req_queue.len());
        });
        
        notify_chan.send(()); // Send incoming notification to responder task.
    }
    
    // TODO: Smarter Scheduling.
    fn dequeue_static_file_request(&mut self) {
        let outer_req_queue_get = self.request_queue_arc.clone();
        let outer_stream_map_get = self.stream_map_arc.clone();
        let outer_cache_get = self.cache_arc.clone();

        let mut notify_channels : ~[Chan<()>] = ~[];

        for _ in range(0, NUM_PROCESS) {
            let req_queue_get = outer_req_queue_get.clone();
            let stream_map_get = outer_stream_map_get.clone();
            let cache_get = outer_cache_get.clone();

            let (notify_port, notify_chan) = Chan::new();
            notify_channels.push(notify_chan);

            spawn(proc() {
                // Port<> cannot be sent to another task. So we have to make this task as the main task that can access self.notify_port.
                
                let (request_port, request_chan) = Chan::new();
                loop {
                   // waiting for new request enqueued.
                    notify_port.recv();

                    req_queue_get.access( |req_queue| {
                        let the_req = req_queue.maybe_pop();
                        match the_req { // FIFO queue.
                            None => { /* do nothing */ }
                            Some(req) => {
                                request_chan.send(req);
                                debug!("A new request dequeued, now the length of queue is {:u}.", req_queue.len());
                            }
                        }
                    });
                    
                    let request = request_port.recv();
                    
                    // Get stream from hashmap.
                    // Use unsafe method, because TcpStream in Rust 0.9 doesn't have "Freeze" bound.
                    let (stream_port, stream_chan) = Chan::new();
                    unsafe {
                        stream_map_get.unsafe_access(|local_stream_map| {
                            let stream = local_stream_map.pop(&request.peer_name).expect("no option tcpstream");
                            stream_chan.send(stream);
                        });
                    }
                    
                    // TODO: Spawning more tasks to respond the dequeued requests concurrently. You may need a semophore to control the concurrency.
                    let stream = stream_port.recv();
                    WebServer::respond_with_static_file(stream, request.path, cache_get.clone());
                    // Close stream automatically.
                    debug!("=====Terminated connection from [{:s}].=====", request.peer_name);
                }
            });
        }

        // Like a Channel Mutex
        loop {
            // Send the requests to each channel
            // Wait for Notification to arrive
            self.notify_port.recv();
            for a_notify_chan in notify_channels.iter() {
                a_notify_chan.send(());
            } 
        }
    }
    
    fn get_peer_name(stream: &mut Option<std::io::net::tcp::TcpStream>) -> ~str {
        match *stream {
            Some(ref mut s) => {
                         match s.peer_name() {
                            Some(pn) => {pn.to_str()},
                            None => (~"")
                         }
                       },
            None => (~"")
        }
    }
}

fn get_args() -> (~str, uint, ~str) {
    fn print_usage(program: &str) {
        println!("Usage: {:s} [options]", program);
        println!("--ip     \tIP address, \"{:s}\" by default.", IP);
        println!("--port   \tport number, \"{:u}\" by default.", PORT);
        println!("--www    \tworking directory, \"{:s}\" by default", WWW_DIR);
        println("-h --help \tUsage");
    }
    
    /* Begin processing program arguments and initiate the parameters. */
    let args = os::args();
    let program = args[0].clone();
    
    let opts = ~[
        getopts::optopt("ip"),
        getopts::optopt("port"),
        getopts::optopt("www"),
        getopts::optflag("h"),
        getopts::optflag("help")
    ];

    let matches = match getopts::getopts(args.tail(), opts) {
        Ok(m) => { m }
        Err(f) => { fail!(f.to_err_msg()) }
    };

    if matches.opt_present("h") || matches.opt_present("help") {
        print_usage(program);
        unsafe { libc::exit(1); }
    }
    
    let ip_str = if matches.opt_present("ip") {
                    matches.opt_str("ip").expect("invalid ip address?").to_owned()
                 } else {
                    IP.to_owned()
                 };
    
    let port:uint = if matches.opt_present("port") {
                        from_str::from_str(matches.opt_str("port").expect("invalid port number?")).expect("not uint?")
                    } else {
                        PORT
                    };
    
    let www_dir_str = if matches.opt_present("www") {
                        matches.opt_str("www").expect("invalid www argument?") 
                      } else { WWW_DIR.to_owned() };
    
    (ip_str, port, www_dir_str)
}

fn main() {
    let (ip_str, port, www_dir_str) = get_args();
    let mut zhtta = WebServer::new(ip_str, port, www_dir_str);
    zhtta.run();
}

// GASH
// ------------------------------------------------------------------------------------------------------------------------------------------------------------------

pub fn run_cmdline(cmd_line: &str) -> ~str{
    let cmd_line: ~str = cmd_line.trim().to_owned();
    
    // handle pipelines 
    let progs: ~[~str] =
        cmd_line.split('|').filter_map(|x| if x != "" { Some(x.to_owned()) } else { None }).to_owned_vec();
    
    let mut pipes: ~[os::Pipe] = ~[];
    
    // create pipes
    pipes.push(os::Pipe { input: 0, out: 0 }); // first pipe is standard input
    for _ in range(0, progs.len() - 1) {
        pipes.push(os::pipe());
    }
    pipes.push(os::pipe()); // last is not necessarily the standard output, for ps3
    
    for i in range(0, progs.len()) {
        run_single_cmd(progs[i], pipes[i].input, pipes[i+1].out, 2, 
                            if (i == progs.len() - 1) { false } else { true }); // all in bg except possibly last one
    }
    // read output from the last pipe.input.
     
    let mut pipe_stream = PipeStream::open(pipes[progs.len()].input);
    let content = pipe_stream.read_to_str();
    return content;
}

// run a single command line, probably with redirection sign >, definitly without pipelines | and background sign &.
fn run_single_cmd(cmd_line: &str, pipe_in: libc::c_int, pipe_out: libc::c_int, pipe_err: libc::c_int, bg: bool) {
    let mut argv = parse_argv(cmd_line);

    if argv.len() <= 0 {
        // empty command line
        return;
    }
    
    let mut out_fd = pipe_out;
    let mut in_fd = pipe_in;
    let err_fd = pipe_err;
    
    let mut i = 0;
    
    while (i < argv.len()) {
        if (argv[i] == ~">") {
            argv.remove(i);
            out_fd = get_fd(argv.remove(i), "w");
        } else if (argv[i] == ~"<") {
            argv.remove(i);
            in_fd = get_fd(argv.remove(i), "r");
        }
        i += 1;
    }
    
    let out_fd = out_fd;
    let in_fd = in_fd;
    
    if argv.len() <= 0 {
        // invalid command line
        return;
    }
    
    let program = argv.remove(0);
    match program {
        ~"cd"       => { if argv.len()>0 { os::change_dir(&path::Path::new(argv[0])); } }
        _           => { if !cmd_exists(program) {
                             println!("{:s}: command not found", program);
                             return;
                         } else {
                             // To see debug! outputs set the RUST_LOG environment variable, e.g.: export RUST_LOG="gash=debug" 
                             debug!("Program: {:s}, in_fd: {:d}, out_fd: {:d}, err_fd: {:d}", program, in_fd, out_fd, err_fd);
                             let opt_prog = run::Process::new(program, argv, 
                                                              run::ProcessOptions { env: None, dir: None,
                                                                                    in_fd: Some(in_fd), out_fd: Some(out_fd), err_fd: Some(err_fd)
                                                                                  });
                                
                             let mut prog = opt_prog.expect("Error: creating process error.");
                             if in_fd != 0 {os::close(in_fd);}
                             if out_fd != 1 {os::close(out_fd);}
                             if err_fd != 2 {os::close(err_fd);}

                             if !bg {
                                 prog.finish();
                                 std::io::stdio::flush();
                                 debug!("Terminated fg program: {:}", program);
                             } else {
                                 let (p_port, p_chan) = Chan::new();
                                 p_chan.send(prog);
                                 spawn(proc() {
                                    let mut prog: run::Process = p_port.recv();
                                       
                                    prog.finish(); 
                                    std::io::stdio::flush();
                                    debug!("Terminated bg program: {:}", program);
                                 });
                            }
                        }
                  }
    } // match program
} // run_single_cmd
    
// input: a single command line
// output: a vector of arguments. The program name is put in the first position.
// notes: arguments can be separated by space(s), ""  
fn parse_argv(cmd_line: &str) -> ~[~str] {
    let mut argv: ~[~str] = ~[];
    let group: ~[~str] = cmd_line.split('\"').filter_map(|x| if x != "" { Some(x.to_owned()) } else { None }).to_owned_vec();

    for i in range(0, group.len()) {            
        if i % 2 == 0 { // split by " "
            argv.push_all_move(group[i].split(' ').filter_map(|x| if x != "" { Some(x.to_owned()) } else { None }).to_owned_vec());
        } else {
            argv.push(group[i].clone());
        }
    }
    
    argv
}

fn cmd_exists(cmd_path: &str) -> bool {
    run::process_output("which", [cmd_path.to_owned()]).expect("exit code error.").status.success()
}

fn get_fd(fpath: &str, mode: &str) -> libc::c_int {
    unsafe {
        let fpathbuf = fpath.to_c_str().unwrap();
        let modebuf = mode.to_c_str().unwrap();
        return libc::fileno(libc::fopen(fpathbuf, modebuf));
    }
}
