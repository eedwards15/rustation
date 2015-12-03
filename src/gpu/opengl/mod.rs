use sdl2;
use sdl2::video::GLProfile;

use glium_sdl2;

use glium::{Program, VertexBuffer, Surface, DrawParameters, Rect, Blend};
use glium::index;
use glium::uniforms::{UniformsStorage, EmptyUniforms};
use glium::program::ProgramCreationInput;
use glium::texture::{Texture2d, UncompressedFloatFormat, MipmapsOption};

/// Maximum number of vertex that can be stored in an attribute
/// buffers
const VERTEX_BUFFER_LEN: u32 = 64 * 1024;

/// Vertex definition used by the draw commands
#[derive(Copy,Clone,Debug)]
pub struct CommandVertex {
    /// Position in PlayStation VRAM coordinates
    pub position: [i16; 2],
    /// RGB color, 8bits per component
    pub color: [u8; 3],
    /// Vertex alpha value, used for blending.
    ///
    /// XXX This is not accurate, we should implement blending
    /// ourselves taking the current semi-transparency mode into
    /// account. We should maybe store two variables, one with the
    /// source factor and one with the destination factor.
    pub alpha: f32,
}

implement_vertex!(CommandVertex, position, color, alpha);

impl CommandVertex {
    pub fn new(pos: [i16; 2],
               color: [u8; 3],
               semi_transparent: bool) -> CommandVertex {
        let alpha =
            if semi_transparent {
                0.5
            } else {
                1.0
            };

        CommandVertex {
            position: pos,
            color: color,
            alpha: alpha,
        }
    }
}

pub struct Renderer {
    /// Glium display
    window: glium_sdl2::SDL2Facade,
    /// Texture used as the target (bound to a framebuffer object) for
    /// the render commands.
    fb_out: Texture2d,
    /// Framebuffer horizontal resolution (native: 1024)
    fb_x_res: u16,
    /// Framebuffer vertical resolution (native: 512)
    fb_y_res: u16,
    /// Program used to process draw commands
    command_program: Program,
    /// Permanent vertex buffer used to store pending draw commands
    command_vertex_buffer: VertexBuffer<CommandVertex>,
    /// Current number or vertices in the command buffer
    nvertices: u32,
    /// List of queued draw commands. Each command contains a
    /// primitive type (triangle or line) and a number of *vertices*
    /// to be drawn from the `vertex_buffer`.
    command_queue: Vec<(index::PrimitiveType, u32)>,
    /// Current draw command. Will be pushed onto the `command_queue`
    /// if a new command needs to be started.
    current_command: (index::PrimitiveType, u32),
    /// Uniforms used by draw commands
    command_uniforms: UniformsStorage<'static, [i32; 2], EmptyUniforms>,
    /// Current draw offset
    offset: (i16, i16),
    /// Parameters for draw commands
    command_params: DrawParameters<'static>,
    /// Program used to display the visible part of the framebuffer
    output_program: Program,
}

impl Renderer {

    pub fn new(sdl_context: &sdl2::Sdl) -> Renderer {
        use glium_sdl2::DisplayBuild;
        // Native PSX VRAM resolution
        let fb_x_res = 1024u32;
        let fb_y_res = 512u32;
        // Internal format for the framebuffer. The real console uses
        // RGB 555 + one "mask" bit which we store as alpha.
        let fb_format = UncompressedFloatFormat::U5U5U5U1;


        // Video output resolution ("TV screen" size). It's not
        // directly related to the internal framebuffer resolution.
        // Only a game-configured fraction of the framebuffer is
        // displayed at any given moment, several display modes are
        // supported by the console.
        let output_width = 1024;
        let output_height = 768;

        let video_subsystem = sdl_context.video().unwrap();

        let gl_attr = video_subsystem.gl_attr();
        gl_attr.set_context_version(3, 3);
        gl_attr.set_context_profile(GLProfile::Core);

        // XXX Debug context is likely to be slower, we should make
        // that configurable at some point.
        gl_attr.set_context_flags().debug().set();

        let window =
            video_subsystem.window("Rustation", output_width, output_height)
            .position_centered()
            .build_glium()
            .ok().expect("Can't create SDL2 window");

        // Build the program used to render GPU primitives in the
        // framebuffer
        let command_vs_src = include_str!("shaders/command_vertex.glsl");
        let command_fs_src = include_str!("shaders/command_fragment.glsl");

        let command_program =
            Program::new(&window,
                         ProgramCreationInput::SourceCode {
                             vertex_shader: &command_vs_src,
                             tessellation_control_shader: None,
                             tessellation_evaluation_shader: None,
                             geometry_shader: None,
                             fragment_shader: &command_fs_src,
                             transform_feedback_varyings: None,
                             // Don't mess with the color correction
                             outputs_srgb: true,
                             uses_point_size: false,
                         }).unwrap();

        let command_vertex_buffer =
            VertexBuffer::empty_persistent(&window,
                                           VERTEX_BUFFER_LEN as usize)
            .unwrap();

        let command_uniforms = uniform! {
            offset: [0; 2],
        };

        // In order to have the line size scale with the internal
        // resolution upscale we need to compute the upscaling ratio.
        //
        // XXX I only use the y scaling factor since I assume that
        // both dimensions are scaled by the same ratio. Otherwise
        // we'd have to change the line thickness depending on its
        // angle and that would be tricky.
        let scaling_factor = fb_y_res as f32 / 512.;

        let command_params = DrawParameters {
            // Default to full screen
            scissor: Some(Rect {
                left: 0,
                bottom: 0,
                width: fb_x_res,
                height: fb_y_res,
            }),
            line_width: Some(scaling_factor),
            // XXX temporary hack for semi-transparency, use basic
            // alpha blending.
            blend: Blend::alpha_blending(),
            ..Default::default()
        };

        // The framebuffer starts uninitialized
        let default_color = Some((0.5, 0.2, 0.1, 0.0));

        let fb_out = Texture2d::empty_with_format(&window,
                                                  fb_format,
                                                  MipmapsOption::NoMipmap,
                                                  fb_x_res,
                                                  fb_y_res).unwrap();

        fb_out.as_surface().clear(None, default_color, false, None, None);

        // Build the program used to render the framebuffer onto the output
        let output_vs_src = include_str!("shaders/output_vertex.glsl");
        let output_fs_src = include_str!("shaders/output_fragment.glsl");

        let output_program =
            Program::new(&window,
                         ProgramCreationInput::SourceCode {
                             vertex_shader: &output_vs_src,
                             tessellation_control_shader: None,
                             tessellation_evaluation_shader: None,
                             geometry_shader: None,
                             fragment_shader: &output_fs_src,
                             transform_feedback_varyings: None,
                             // Don't mess with the color correction.
                             // XXX We should probably do manual color
                             // correction to match the real console's
                             // output colors
                             outputs_srgb: true,
                             uses_point_size: false,
                         }).unwrap();

        Renderer {
            window: window,
            fb_out: fb_out,
            fb_x_res: fb_x_res as u16,
            fb_y_res: fb_y_res as u16,
            command_program: command_program,
            command_vertex_buffer: command_vertex_buffer,
            nvertices: 0,
            command_queue: Vec::new(),
            current_command: (index::PrimitiveType::TrianglesList, 0),
            command_uniforms: command_uniforms,
            offset: (0, 0),
            command_params: command_params,
            output_program: output_program,
        }
    }

    /// Add a triangle to the draw buffer
    pub fn push_triangle(&mut self, vertices: &[CommandVertex; 3]) {
        self.push_primitive(index::PrimitiveType::TrianglesList,
                            vertices);
    }

    /// Add a quad to the draw buffer
    pub fn push_quad(&mut self, vertices: &[CommandVertex; 4]) {
        self.push_triangle(&[vertices[0], vertices[1], vertices[2]]);
        self.push_triangle(&[vertices[1], vertices[2], vertices[3]]);
    }

    /// Add a line to the draw buffer
    pub fn push_line(&mut self, vertices: &[CommandVertex; 2]) {
        self.push_primitive(index::PrimitiveType::LinesList,
                            vertices);
    }

    /// Add a primitive to the draw buffer
    fn push_primitive(&mut self,
                      primitive_type: index::PrimitiveType,
                      vertices: &[CommandVertex]) {
        let primitive_vertices = vertices.len() as u32;

        // Make sure we have enough room left to queue the vertex. We
        // need to push two triangles to draw a quad, so 6 vertex
        if self.nvertices + primitive_vertices > VERTEX_BUFFER_LEN {
            // The vertex attribute buffers are full, force an early
            // draw
            self.draw();
        }

        let (mut cmd_type, mut cmd_len) = self.current_command;

        if primitive_type != cmd_type {
            // We have to change the primitive type. Push the current
            // command onto the queue and start a new one.
            if cmd_len > 0 {
                self.command_queue.push(self.current_command);
            }

            cmd_type = primitive_type;
            cmd_len = 0;
        }

        // Copy the vertices into the vertex buffer
        let start = self.nvertices as usize;
        let end = start + primitive_vertices as usize;

        let slice = self.command_vertex_buffer.slice(start..end).unwrap();
        slice.write(vertices);

        self.nvertices += primitive_vertices;
        self.current_command = (cmd_type, cmd_len + primitive_vertices);
    }

    /// Fill a rectangle in memory with the given color. This method
    /// ignores the mask bit, the drawing area and the drawing offset.
    pub fn fill_rect(&mut self,
                     color: [u8; 3],
                     top: u16, left: u16,
                     bottom: u16, right: u16) {
        // Flush any pending draw commands
        self.draw();

        // Save the current value of the scissor
        let scissor = self.command_params.scissor;

        // Disable the scissor and offset
        self.command_params.scissor = None;
        self.command_uniforms = uniform! {
            offset: [0; 2],
        };

        let top = top as i16;
        let left = left as i16;
        // Fill rect is inclusive
        let bottom = bottom as i16;
        let right = right as i16;

        // Draw a quad to fill the rectangle
        self.push_quad(&[
            CommandVertex::new([left, top], color, false),
            CommandVertex::new([right, top], color, false),
            CommandVertex::new([left, bottom], color, false),
            CommandVertex::new([right, bottom], color, false),
            ]);

        self.draw();

        // Restore previous scissor box and offset
        self.command_params.scissor = scissor;

        let (x, y) = self.offset;
        self.command_uniforms = uniform! {
            offset: [x as i32, y as i32],
        };
    }

    /// Set the value of the uniform draw offset
    pub fn set_draw_offset(&mut self, x: i16, y: i16) {
        // Force draw for the primitives with the current offset
        self.draw();

        self.offset = (x, y);

        self.command_uniforms = uniform! {
            offset : [x as i32, y as i32],
        }
    }

    /// Set the drawing area. Coordinates are offsets in the
    /// PlayStation VRAM
    pub fn set_drawing_area(&mut self,
                            left: u16, top: u16,
                            right: u16, bottom: u16) {
        // Render any pending primitives
        self.draw();

        let (left, top) = self.scale_coords(left, top);
        let (right, bottom) = self.scale_coords(right, bottom);

        if left > right || bottom > top {
            // XXX What should we do here? This happens often because
            // the drawing area is set in two successive calls to set
            // the top_left and then bottom_right so the intermediate
            // value is often wrong.
            self.command_params.scissor = Some(Rect {
                left: 0,
                bottom: 0,
                width: 0,
                height: 0,
            });
        } else {
            // Width and height are inclusive
            let width = right - left + 1;
            let height = top - bottom + 1;

            self.command_params.scissor = Some(Rect {
                left: left,
                bottom: bottom,
                width: width,
                height: height,
            });
        }
    }

    /// Draw the buffered commands and reset the buffers
    pub fn draw(&mut self) {

        // Push the last pending command if needed
        let (_, cmd_len) = self.current_command;

        if cmd_len > 0 {
            self.command_queue.push(self.current_command);
        }

        if self.command_queue.is_empty() {
            // Nothing to be done
            return;
        }

        let mut surface = self.fb_out.as_surface();

        let mut vertex_pos = 0;

        for &(cmd_type, cmd_len) in &self.command_queue {
            let start = vertex_pos;
            let end = start + cmd_len as usize;

            let vertices =
                self.command_vertex_buffer.slice(start..end)
                .unwrap();

            surface.draw(vertices,
                         &index::NoIndices(cmd_type),
                         &self.command_program,
                         &self.command_uniforms,
                         &self.command_params).unwrap();

            vertex_pos = end;
        }

        // Reset the buffers
        self.nvertices = 0;
        self.command_queue.clear();
        self.current_command = (index::PrimitiveType::TrianglesList, 0);
    }

    /// Draw the buffered commands and refresh the video output.
    pub fn display(&mut self,
                   fb_x: u16, fb_y: u16,
                   width: u16, height: u16) {
        // Draw any pending commands
        self.draw();

        let params = DrawParameters {
            blend: Blend::alpha_blending(),
            ..Default::default()
        };

        let mut frame = self.window.draw();


        // We sample `fb_out` onto the screen
        let uniforms = uniform! {
            fb: &self.fb_out,
            alpha: 1.0f32,
        };

        /// Vertex definition for the video output program
        #[derive(Copy, Clone)]
        struct Vertex {
            /// Vertex position on the screen
            position: [f32; 2],
            /// Corresponding coordinate in the framebuffer
            fb_coord: [u16; 2],
        }

        implement_vertex!(Vertex, position, fb_coord);

        let fb_x_start = fb_x;
        let fb_x_end = fb_x + width;
        // OpenGL puts the Y axis in the opposite direction compared
        // to the PlayStation GPU coordinate system so we must start
        // at the bottom here.
        let fb_y_start = fb_y + height;
        let fb_y_end = fb_y;

        // We render a single quad containing the texture to the
        // screen
        let vertices =
            VertexBuffer::new(&self.window,
                              &[Vertex { position: [-1.0, -1.0],
                                         fb_coord: [fb_x_start, fb_y_start] },
                                Vertex { position: [1.0, -1.0],
                                         fb_coord: [fb_x_end, fb_y_start] },
                                Vertex { position: [-1.0, 1.0],
                                         fb_coord: [fb_x_start, fb_y_end] },
                                Vertex { position: [1.0, 1.0],
                                         fb_coord: [fb_x_end, fb_y_end] }])
            .unwrap();

        frame.draw(&vertices,
                   &index::NoIndices(index::PrimitiveType::TriangleStrip),
                   &self.output_program,
                   &uniforms,
                   &params).unwrap();


        // Draw the full framebuffer at the bottom right transparently
        // We sample `fb_out` onto the screen
        let vertices =
            VertexBuffer::new(&self.window,
                              &[Vertex { position: [0., -1.0],
                                         fb_coord: [0, 511] },
                                Vertex { position: [1.0, -1.0],
                                         fb_coord: [1024, 511] },
                                Vertex { position: [0., -0.5],
                                         fb_coord: [0, 0] },
                                Vertex { position: [1.0, -0.5],
                                         fb_coord: [1024, 0] }])
            .unwrap();

        let uniforms = uniform! {
            fb: &self.fb_out,
            alpha: 0.5f32,
        };

        frame.draw(&vertices,
                   &index::NoIndices(index::PrimitiveType::TriangleStrip),
                   &self.output_program,
                   &uniforms,
                   &params).unwrap();

        // Flip the buffers and display the new frame
        frame.finish().unwrap();
    }

    /// Convert coordinates in the PlayStation framebuffer to
    /// coordinates in our potentially scaled OpenGL
    /// framebuffer. Coordinates are rounded to the nearest pixel.
    fn scale_coords(&self, x: u16, y: u16) -> (u32, u32) {
        // OpenGL has (0, 0) at the bottom left, the PSX at the top
        // left so we need to complement the y coordinate
        let y = !y & 0x1ff;

        let x = (x as u32 * self.fb_x_res as u32 + 512) / 1024;
        let y = (y as u32 * self.fb_y_res as u32 + 256) / 512;

        (x, y)
    }
}
