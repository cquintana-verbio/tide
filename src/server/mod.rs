//! An HTTP server

use futures::future::{self, BoxFuture};
use http_service::HttpService;
use std::sync::Arc;

use crate::{
    middleware::{Middleware, Next},
    router::{Router, Selection},
    Request
};

mod route;

pub use route::Route;

/// An HTTP server.
///
/// Servers are built up as a combination of *state*, *endpoints* and *middleware*:
///
/// - Server state is user-defined, and is provided via the [`Server::with_state`] function. The
/// state is available as a shared reference to all app endpoints.
///
/// - Endpoints provide the actual application-level code corresponding to
/// particular URLs. The [`Server::at`] method creates a new *route* (using
/// standard router syntax), which can then be used to register endpoints
/// for particular HTTP request types.
///
/// - Middleware extends the base Tide framework with additional request or
/// response processing, such as compression, default headers, or logging. To
/// add middleware to an app, use the [`Server::middleware`] method.
///
/// # Hello, world!
///
/// You can start a simple Tide application that listens for `GET` requests at path `/hello`
/// on `127.0.0.1:8000` with:
///
/// ```rust, no_run
///
/// let mut app = tide::Server::new();
/// app.at("/hello").get(|_| async move {"Hello, world!"});
/// app.run("127.0.0.1:8000").unwrap();
/// ```
///
/// # Routing and parameters
///
/// Tide's routing system is simple and similar to many other frameworks. It
/// uses `:foo` for "wildcard" URL segments, and `*foo` to match the rest of a
/// URL (which may include multiple segments). Here's an example using wildcard
/// segments as parameters to endpoints:
///
/// ```rust, no_run
/// use tide::error::ResultExt;
///
/// async fn hello(cx: tide::Request<()>) -> tide::Result<String> {
///     let user: String = cx.param("user").client_err()?;
///     Ok(format!("Hello, {}!", user))
/// }
///
/// async fn goodbye(cx: tide::Request<()>) -> tide::Result<String> {
///     let user: String = cx.param("user").client_err()?;
///     Ok(format!("Goodbye, {}.", user))
/// }
///
/// let mut app = tide::Server::new();
///
/// app.at("/hello/:user").get(hello);
/// app.at("/goodbye/:user").get(goodbye);
/// app.at("/").get(|_| async move {
///     "Use /hello/{your name} or /goodbye/{your name}"
/// });
///
/// app.run("127.0.0.1:8000").unwrap();
/// ```
///
/// You can learn more about routing in the [`Server::at`] documentation.
///
/// # Serverlication state
///
/// ```rust, no_run
///
/// use http::status::StatusCode;
/// use serde::{Deserialize, Serialize};
/// use std::sync::Mutex;
/// use tide::{error::ResultExt, response, Server, Request, Result};
///
/// #[derive(Default)]
/// struct Database {
///     contents: Mutex<Vec<Message>>,
/// }
///
/// #[derive(Serialize, Deserialize, Clone)]
/// struct Message {
///     author: Option<String>,
///     contents: String,
/// }
///
/// impl Database {
///     fn insert(&self, msg: Message) -> usize {
///         let mut table = self.contents.lock().unwrap();
///         table.push(msg);
///         table.len() - 1
///     }
///
///     fn get(&self, id: usize) -> Option<Message> {
///         self.contents.lock().unwrap().get(id).cloned()
///     }
/// }
///
/// async fn new_message(mut cx: Request<Database>) -> Result<String> {
///     let msg = cx.body_json().await.client_err()?;
///     Ok(cx.state().insert(msg).to_string())
/// }
///
/// async fn get_message(cx: Request<Database>) -> Result {
///     let id = cx.param("id").client_err()?;
///     if let Some(msg) = cx.state().get(id) {
///         Ok(response::json(msg))
///     } else {
///         Err(StatusCode::NOT_FOUND)?
///     }
/// }
///
/// fn main() {
///     let mut app = Server::with_state(Database::default());
///     app.at("/message").post(new_message);
///     app.at("/message/:id").get(get_message);
///     app.run("127.0.0.1:8000").unwrap();
/// }
/// ```
#[allow(missing_debug_implementations)]
pub struct Server<State> {
    router: Router<State>,
    middleware: Vec<Arc<dyn Middleware<State>>>,
    state: State,
}

impl Server<()> {
    /// Create an empty `Server`, with no initial middleware or configuration.
    pub fn new() -> Server<()> {
        Self::with_state(())
    }
}

impl Default for Server<()> {
    fn default() -> Server<()> {
        Self::new()
    }
}

impl<State: Send + Sync + 'static> Server<State> {
    /// Create an `Server`, with initial middleware or configuration.
    pub fn with_state(state: State) -> Server<State> {
        Server {
            router: Router::new(),
            middleware: Vec::new(),
            state,
        }
    }

    /// Add a new route at the given `path`, relative to root.
    ///
    /// Routing means mapping an HTTP request to an endpoint. Here Tide applies
    /// a "table of contents" approach, which makes it easy to see the overall
    /// app structure. Endpoints are selected solely by the path and HTTP method
    /// of a request: the path determines the resource and the HTTP verb the
    /// respective endpoint of the selected resource. Example:
    ///
    /// ```rust,no_run
    /// # let mut app = tide::Server::new();
    /// app.at("/").get(|_| async move {"Hello, world!"});
    /// ```
    ///
    /// A path is comprised of zero or many segments, i.e. non-empty strings
    /// separated by '/'. There are two kinds of segments: concrete and
    /// wildcard. A concrete segment is used to exactly match the respective
    /// part of the path of the incoming request. A wildcard segment on the
    /// other hand extracts and parses the respective part of the path of the
    /// incoming request to pass it along to the endpoint as an argument. A
    /// wildcard segment is written as `:name`, which creates an endpoint
    /// parameter called `name`. It is not possible to define wildcard segments
    /// with different names for otherwise identical paths.
    ///
    /// Alternatively a wildcard definitions can start with a `*`, for example
    /// `*path`, which means that the wildcard will match to the end of given
    /// path, no matter how many segments are left, even nothing.
    ///
    /// The name of the parameter can be omitted to define a path that matches
    /// the required structure, but where the parameters are not required.
    /// `:` will match a segment, and `*` will match an entire path.
    ///
    /// Here are some examples omitting the HTTP verb based endpoint selection:
    ///
    /// ```rust,no_run
    /// # let mut app = tide::Server::new();
    /// app.at("/");
    /// app.at("/hello");
    /// app.at("add_two/:num");
    /// app.at("files/:user/*");
    /// app.at("static/*path");
    /// app.at("static/:context/:");
    /// ```
    ///
    /// There is no fallback route matching, i.e. either a resource is a full
    /// match or not, which means that the order of adding resources has no
    /// effect.
    pub fn at<'a>(&'a mut self, path: &'a str) -> Route<'a, State> {
        Route::new(&mut self.router, path.to_owned())
    }

    /// Add middleware to an application.
    ///
    /// Middleware provides application-global customization of the
    /// request/response cycle, such as compression, logging, or header
    /// modification. Middleware is invoked when processing a request, and can
    /// either continue processing (possibly modifying the response) or
    /// immediately return a response. See the [`Middleware`] trait for details.
    ///
    /// Middleware can only be added at the "top level" of an application,
    /// and is processed in the order in which it is applied.
    pub fn middleware(&mut self, m: impl Middleware<State>) -> &mut Self {
        self.middleware.push(Arc::new(m));
        self
    }

    /// Make this app into an `HttpService`.
    ///
    /// This lower-level method lets you host a Tide application within an HTTP
    /// server of your choice, via the `http_service` interface crate.
    pub fn into_http_service(self) -> Service<State> {
        Service {
            router: Arc::new(self.router),
            state: Arc::new(self.state),
            middleware: Arc::new(self.middleware),
        }
    }

    /// Start Running the app at the given address.
    ///
    /// Blocks the calling thread indefinitely.
    #[cfg(feature = "hyper")]
    #[doc(hidden)]
    // TODO: remove this API
    pub fn run(self, addr: impl std::net::ToSocketAddrs) -> std::io::Result<()> {
        let addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or(std::io::ErrorKind::InvalidInput)?;

        http_service_hyper::run(self.into_http_service(), addr);
        Ok(())
    }

    /// Asynchronously serve the app at the given address.
    #[cfg(feature = "hyper")]
    pub async fn listen(self, addr: impl std::net::ToSocketAddrs) -> std::io::Result<()> {
        // TODO: try handling all addresses
        let addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or(std::io::ErrorKind::InvalidInput)?;

        println!("Service is listening on: http://{}", addr);
        let res = http_service_hyper::serve(self.into_http_service(), addr).await;
        res.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }
}

/// An instantiated Tide server.
///
/// This type is useful only in conjunction with the [`HttpService`] trait,
/// i.e. for hosting a Tide app within some custom HTTP server.
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct Service<State> {
    router: Arc<Router<State>>,
    state: Arc<State>,
    middleware: Arc<Vec<Arc<dyn Middleware<State>>>>,
}

impl<State: Sync + Send + 'static> HttpService for Service<State> {
    type Connection = ();
    type ConnectionFuture = future::Ready<Result<(), std::io::Error>>;
    type ResponseFuture = BoxFuture<'static, Result<http_service::Response, std::io::Error>>;

    fn connect(&self) -> Self::ConnectionFuture {
        future::ok(())
    }

    fn respond(&self, _conn: &mut (), req: http_service::Request) -> Self::ResponseFuture {
        let path = req.uri().path().to_owned();
        let method = req.method().to_owned();
        let router = self.router.clone();
        let middleware = self.middleware.clone();
        let state = self.state.clone();

        Box::pin(async move {
            let fut = {
                let Selection { endpoint, params } = router.route(&path, method);
                let cx = Request::new(state, req, params);

                let next = Next {
                    endpoint,
                    next_middleware: &middleware,
                };

                next.run(cx)
            };

            Ok(fut.await)
        })
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use std::sync::Arc;

    use super::*;
    use crate::{middleware::Next, router::Selection, Request, Response};

    fn simulate_request<'a, State: Default + Clone + Send + Sync + 'static>(
        app: &'a Server<State>,
        path: &'a str,
        method: http::Method,
    ) -> BoxFuture<'a, Response> {
        let Selection { endpoint, params } = app.router.route(path, method.clone());

        let state = Arc::new(State::default());
        let req = http::Request::builder()
            .method(method)
            .body(http_service::Body::empty())
            .unwrap();
        let cx = Request::new(state, req, params);
        let next = Next {
            endpoint,
            next_middleware: &app.middleware,
        };

        next.run(cx)
    }

    #[test]
    fn simple_static() {
        let mut router = Server::new();
        router.at("/").get(|_| async move { "/" });
        router.at("/foo").get(|_| async move { "/foo" });
        router.at("/foo/bar").get(|_| async move { "/foo/bar" });

        for path in &["/", "/foo", "/foo/bar"] {
            let res = block_on(simulate_request(&router, path, http::Method::GET));
            let body = block_on(res.into_body().into_vec()).expect("Reading body should succeed");
            assert_eq!(&*body, path.as_bytes());
        }
    }

    #[test]
    fn nested_static() {
        let mut router = Server::new();
        router.at("/a").get(|_| async move { "/a" });
        router.at("/b").nest(|router| {
            router.at("/").get(|_| async move { "/b" });
            router.at("/a").get(|_| async move { "/b/a" });
            router.at("/b").get(|_| async move { "/b/b" });
            router.at("/c").nest(|router| {
                router.at("/a").get(|_| async move { "/b/c/a" });
                router.at("/b").get(|_| async move { "/b/c/b" });
            });
            router.at("/d").get(|_| async move { "/b/d" });
        });
        router.at("/a/a").nest(|router| {
            router.at("/a").get(|_| async move { "/a/a/a" });
            router.at("/b").get(|_| async move { "/a/a/b" });
        });
        router.at("/a/b").nest(|router| {
            router.at("/").get(|_| async move { "/a/b" });
        });

        for failing_path in &["/", "/a/a", "/a/b/a"] {
            let res = block_on(simulate_request(&router, failing_path, http::Method::GET));
            if !res.status().is_client_error() {
                panic!(
                    "Should have returned a client error when router cannot match with path {}",
                    failing_path
                );
            }
        }

        for path in &[
            "/a", "/a/a/a", "/a/a/b", "/a/b", "/b", "/b/a", "/b/b", "/b/c/a", "/b/c/b", "/b/d",
        ] {
            let res = block_on(simulate_request(&router, path, http::Method::GET));
            let body = block_on(res.into_body().into_vec()).expect("Reading body should succeed");
            assert_eq!(&*body, path.as_bytes());
        }
    }

    #[test]
    fn multiple_methods() {
        let mut router = Server::new();
        router.at("/a").nest(|router| {
            router.at("/b").get(|_| async move { "/a/b GET" });
        });
        router.at("/a/b").post(|_| async move { "/a/b POST" });

        for (path, method) in &[("/a/b", http::Method::GET), ("/a/b", http::Method::POST)] {
            let res = block_on(simulate_request(&router, path, method.clone()));
            assert!(res.status().is_success());
            let body = block_on(res.into_body().into_vec()).expect("Reading body should succeed");
            assert_eq!(&*body, format!("{} {}", path, method).as_bytes());
        }
    }
}